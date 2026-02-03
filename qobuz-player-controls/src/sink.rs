use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use rodio::Source;
use rodio::cpal::traits::{DeviceTrait, HostTrait};
use rodio::{decoder::DecoderBuilder, queue::queue};
use tokio::sync::watch::{self, Receiver, Sender};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::error::Error;
use crate::stretch_source_signalsmith::SignalsmithStretchSource;
use crate::{Result, VolumeReceiver};

#[derive(Clone, Copy)]
pub struct PlaybackStretchConfig {
    pub time_stretch_ratio: f32,
    pub pitch_semitones: i16,
    pub pitch_cents: i16,
}

impl Default for PlaybackStretchConfig {
    fn default() -> Self {
        Self {
            time_stretch_ratio: 1.0,
            pitch_semitones: 0,
            pitch_cents: 0,
        }
    }
}

pub struct Sink {
    output_stream: Option<rodio::OutputStream>,
    sink: Option<rodio::Sink>,
    sender: Option<Arc<rodio::queue::SourcesQueueInput>>,
    volume: VolumeReceiver,
    playback_stretch: Arc<RwLock<PlaybackStretchConfig>>,
    live_stretch_enabled: bool,
    track_finished: Sender<()>,
    track_handle: Option<JoinHandle<()>>,
    duration_played: Arc<Mutex<Duration>>,
    position_offset_ms: Arc<Mutex<i64>>,
    selected_device_name: Arc<Mutex<Option<String>>>,
}

impl Sink {
    pub fn new(volume: VolumeReceiver, playback_stretch: Arc<RwLock<PlaybackStretchConfig>>) -> Result<Self> {
        let (track_finished, _) = watch::channel(());
        Ok(Self {
            sink: Default::default(),
            output_stream: Default::default(),
            sender: Default::default(),
            volume,
            playback_stretch,
            live_stretch_enabled: false,
            track_finished,
            track_handle: Default::default(),
            duration_played: Default::default(),
            position_offset_ms: Arc::new(Mutex::new(0)),
            selected_device_name: Arc::new(Mutex::new(None)),
        })
    }

    pub fn set_device(&self, device_name: Option<String>) {
        tracing::info!("Setting audio device to: {:?}", device_name);
        *self.selected_device_name.lock() = device_name;
    }

    pub fn get_device(&self) -> Option<String> {
        self.selected_device_name.lock().clone()
    }

    pub fn track_finished(&self) -> Receiver<()> {
        self.track_finished.subscribe()
    }

    pub fn position(&self) -> Duration {
        let position = self
            .sink
            .as_ref()
            .map(|sink| sink.get_pos())
            .unwrap_or_default();

        let duration_played = *self.duration_played.lock();

        if position < duration_played {
            return Default::default();
        }

        let raw = position - duration_played;
        let raw_ms = raw.as_millis() as i64;
        let offset_ms = *self.position_offset_ms.lock();
        Duration::from_millis(raw_ms.saturating_add(offset_ms).max(0) as u64)
    }

    fn reset_position_adjustment(&self) {
        *self.position_offset_ms.lock() = 0;
    }

    pub fn adjust_position_offset_ms(&self, delta_ms: i64) {
        let mut offset = self.position_offset_ms.lock();
        *offset = offset.saturating_add(delta_ms);
    }

    pub fn play(&self) -> Result<()> {
        if let Some(sink) = &self.sink {
            sink.play();
        }
        Ok(())
    }

    pub fn pause(&self) -> Result<()> {
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        Ok(())
    }

    pub fn seek(&self, duration: Duration) -> Result<()> {
        if let Some(sink) = &self.sink {
            match sink.try_seek(duration) {
                Ok(_) => {
                    *self.duration_played.lock() = Default::default();
                    self.reset_position_adjustment();
                }
                Err(err) => return Err(err.into()),
            };
        }

        Ok(())
    }

    pub fn clear(&mut self) -> Result<()> {
        tracing::info!("Clearing sink");
        self.clear_queue()?;
        self.sink = None;
        self.sender = None;
        self.output_stream = None;
        *self.duration_played.lock() = Default::default();
        self.reset_position_adjustment();

        if let Some(handle) = self.track_handle.take() {
            handle.abort();
        }

        Ok(())
    }

    pub fn clear_queue(&mut self) -> Result<()> {
        tracing::info!("Clearing sink queue");
        *self.duration_played.lock() = Default::default();
        self.reset_position_adjustment();

        if let Some(sender) = self.sender.as_ref() {
            sender.clear();
        };
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.sink.is_none()
    }

    pub fn supports_live_stretch(&self) -> bool {
        self.live_stretch_enabled
    }

    pub fn query_track(
        &mut self,
        track_path: &Path,
        start_at: Option<Duration>,
    ) -> Result<QueryTrackResult> {
        tracing::info!("Sink query track: {}", track_path.to_string_lossy());

        let file = fs::File::open(track_path).map_err(|err| Error::StreamError {
            message: format!("Failed to read file: {track_path:?}: {err}"),
        })?;
        let decoded = DecoderBuilder::new()
            .with_data(file)
            .with_seekable(true)
            .build()?;

        let sample_rate = decoded.sample_rate();
        #[allow(unused_variables)]
        let channels = decoded.channels();
        self.live_stretch_enabled = channels == 2;
        let (mut source, track_duration_override): (
            Box<dyn rodio::Source<Item = f32> + Send>,
            Option<Duration>,
        ) = {
            if channels == 2 {
                let stretch_source =
                    SignalsmithStretchSource::new(decoded, sample_rate, self.playback_stretch.clone());
                (Box::new(stretch_source), None)
            } else {
                (box_source_f32(decoded), None)
            }
        };
        let same_sample_rate = self
            .output_stream
            .as_ref()
            .map(|stream| stream.config().sample_rate() == sample_rate)
            .unwrap_or(true);

        if !same_sample_rate {
            return Ok(QueryTrackResult::RecreateStreamRequired);
        }

        let current_device = self.selected_device_name.lock().clone();
        let needs_stream = self.output_stream.is_none() 
            || self.sink.is_none() 
            || self.sender.is_none();

        if needs_stream {
            let device_to_use = if current_device.is_none() {
                tracing::info!("Default device selected, resolving to system default");
                match get_default_device_name() {
                    Ok(Some(default_name)) => {
                        tracing::info!("Resolved default device to: {}", default_name);
                        Some(default_name)
                    }
                    Ok(None) => {
                        tracing::warn!("No system default device found, using None");
                        None
                    }
                    Err(e) => {
                        tracing::error!("Failed to get default device name: {}", e);
                        None
                    }
                }
            } else {
                current_device
            };
            
            tracing::info!("Creating audio stream with device: {:?}", device_to_use);
            match open_stream_with_device(sample_rate, device_to_use.as_deref()) {
                Ok(mut stream_handle) => {
                    stream_handle.log_on_drop(false);

                    let (sender, receiver) = queue(true);
                    let sink = rodio::Sink::connect_new(stream_handle.mixer());
                    sink.append(receiver);
                    set_volume(&sink, &self.volume.borrow());

                    self.sink = Some(sink);
                    self.sender = Some(sender);
                    self.output_stream = Some(stream_handle);
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    if error_msg.contains("device") || error_msg.contains("no longer available") {
                        tracing::warn!("Selected device unavailable, trying default device");
                        *self.selected_device_name.lock() = None;
                        let default_device = get_default_device_name()
                            .ok()
                            .flatten();
                        match open_stream_with_device(sample_rate, default_device.as_deref()) {
                            Ok(mut stream_handle) => {
                                stream_handle.log_on_drop(false);

                                let (sender, receiver) = queue(true);
                                let sink = rodio::Sink::connect_new(stream_handle.mixer());
                                sink.append(receiver);
                                set_volume(&sink, &self.volume.borrow());

                                self.sink = Some(sink);
                                self.sender = Some(sender);
                                self.output_stream = Some(stream_handle);
                            }
                            Err(fallback_err) => {
                                tracing::error!("Failed to open default device as fallback: {}", fallback_err);
                                return Err(e);
                            }
                        }
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        if let Some(pos) = start_at {
            if pos > Duration::ZERO {
                source.try_seek(pos)?;
            }
        }

        let track_finished = self.track_finished.clone();
        let track_duration = track_duration_override
            .unwrap_or_else(|| source.total_duration().unwrap_or_default());

        let duration_played = self.duration_played.clone();
        let position_offset_ms = self.position_offset_ms.clone();
        let signal = self.sender.as_ref().unwrap().append_with_signal(source);

        let track_handle = tokio::spawn(async move {
            loop {
                if signal.try_recv().is_ok() {
                    *duration_played.lock() += track_duration;
                    *position_offset_ms.lock() = 0;
                    track_finished.send(()).expect("infallible");
                    break;
                }
                sleep(Duration::from_millis(200)).await;
            }
        });

        self.track_handle = Some(track_handle);

        Ok(QueryTrackResult::Queued)
    }

    pub fn sync_volume(&self) {
        if let Some(sink) = &self.sink {
            set_volume(sink, &self.volume.borrow());
        }
    }
}

fn box_source_f32<S>(source: S) -> Box<dyn rodio::Source<Item = f32> + Send>
where
    S: rodio::Source<Item = f32> + Send + 'static,
{
    Box::new(source)
}

fn set_volume(sink: &rodio::Sink, volume: &f32) {
    let volume = volume.clamp(0.0, 1.0).powi(3);
    sink.set_volume(volume);
}

#[allow(dead_code)]
fn open_default_stream(sample_rate: u32) -> Result<rodio::OutputStream> {
    open_stream_with_device(sample_rate, None)
}

fn open_stream_with_device(sample_rate: u32, device_name: Option<&str>) -> Result<rodio::OutputStream> {
    tracing::info!("Opening audio stream with device: {:?}", device_name);
    
    let host = rodio::cpal::default_host();
    
    if let Some(device_name) = device_name {
        tracing::info!("Looking for device: {}", device_name);
        let devices = host.output_devices().map_err(|e| {
            tracing::error!("Failed to enumerate output devices: {}", e);
            Error::StreamError {
                message: format!("Failed to enumerate output devices: {}", e),
            }
        })?;
        
        for device in devices {
            let name = device.name().unwrap_or_else(|_| "Unknown".to_string());
            tracing::debug!("Found device: {}", name);
            
            if name == device_name {
                tracing::info!("Using selected device: {}", name);
                return rodio::OutputStreamBuilder::from_device(device)
                    .and_then(|x| x.with_sample_rate(sample_rate).open_stream_or_fallback())
                    .map_err(|e| {
                        tracing::error!("Failed to open selected device {}: {}", device_name, e);
                        Error::StreamError {
                            message: format!("Failed to open device {}: {}", device_name, e),
                        }
                    });
            }
        }
        
        tracing::warn!("Selected device '{}' not found, falling back to default", device_name);
    }
    
    rodio::OutputStreamBuilder::from_default_device()
        .and_then(|x| x.with_sample_rate(sample_rate).open_stream())
        .or_else(|original_err| {
            tracing::warn!("Failed to open default device, trying any available device");
            let mut devices = match host.output_devices() {
                Ok(devices) => devices,
                Err(e) => {
                    tracing::error!("Failed to enumerate output devices: {}", e);
                    return Err(original_err);
                }
            };

            devices
                .find_map(|d| {
                    let name = d.name().unwrap_or_else(|_| "Unknown".to_string());
                    tracing::debug!("Trying device: {}", name);
                    rodio::OutputStreamBuilder::from_device(d)
                        .and_then(|x| x.with_sample_rate(sample_rate).open_stream_or_fallback())
                        .ok()
                })
                .ok_or(original_err)
        })
        .map_err(|e: rodio::StreamError| {
            tracing::error!("Failed to open any audio device: {}", e);
            Error::StreamError {
                message: format!("Failed to open audio device: {}", e),
            }
        })
}

pub fn list_audio_devices() -> Result<Vec<AudioDevice>> {
    tracing::info!("Listing available audio devices");
    let host = rodio::cpal::default_host();
    let devices = host.output_devices().map_err(|e| {
        tracing::error!("Failed to enumerate output devices: {}", e);
        Error::StreamError {
            message: format!("Failed to enumerate output devices: {}", e),
        }
    })?;
    
    let mut device_list = Vec::new();
    for device in devices {
        let name = device.name().unwrap_or_else(|_| "Unknown Device".to_string());
        tracing::debug!("Found audio device: {}", name);
        device_list.push(AudioDevice { name });
    }
    
    tracing::info!("Found {} audio device(s)", device_list.len());
    Ok(device_list)
}

pub fn get_default_device_name() -> Result<Option<String>> {
    tracing::info!("Getting default audio device name");
    let host = rodio::cpal::default_host();
    match host.default_output_device() {
        Some(device) => {
            let name = device.name().unwrap_or_else(|_| "Unknown Device".to_string());
            tracing::info!("Default audio device: {}", name);
            Ok(Some(name))
        }
        None => {
            tracing::warn!("No default audio device found");
            Ok(None)
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AudioDevice {
    pub name: String,
}

pub enum QueryTrackResult {
    Queued,
    RecreateStreamRequired,
}

impl Drop for Sink {
    fn drop(&mut self) {
        self.clear().unwrap();
    }
}

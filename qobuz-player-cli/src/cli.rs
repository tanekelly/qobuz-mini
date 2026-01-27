use std::{
    io::{Write, stdin, stdout},
    path::PathBuf,
    sync::Arc,
};

use clap::{Parser, Subcommand};
use qobuz_player_controls::{
    AudioQuality, client::Client, database::Database, notification::{Notification, NotificationBroadcast},
    player::Player, list_audio_devices, get_default_device_name,
};
use qobuz_player_rfid::RfidState;
use snafu::prelude::*;
use tokio::sync::broadcast;
use tokio_schedule::{Job, every};

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[clap(short, long)]
    /// Log level
    verbosity: Option<tracing::Level>,

    #[clap(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Default. Starts the player
    Open {
        /// Provide a username (overrides any configured value)
        #[clap(short, long)]
        username: Option<String>,

        #[clap(short, long)]
        /// Provide a password (overrides any configured value)
        password: Option<String>,

        #[clap(short, long)]
        /// Provide max audio quality (overrides any configured value)
        max_audio_quality: Option<AudioQuality>,

        #[clap(short, long, default_value_t = false)]
        /// Disable the TUI interface
        disable_tui: bool,

        #[clap(long, default_value_t = false)]
        /// Disable the album cover image in TUI
        disable_tui_album_cover: bool,

        #[cfg(target_os = "linux")]
        #[clap(long, default_value_t = false)]
        /// Disable the mpris interface
        disable_mpris: bool,

        #[clap(short, long, default_value_t = false)]
        /// Start web server with web api and ui
        web: bool,

        #[clap(long)]
        /// Secret used for web ui auth
        web_secret: Option<String>,

        #[clap(long, default_value_t = 9888)]
        /// Specify port for the web server
        port: u16,

        #[clap(long, default_value_t = false)]
        /// Enable rfid interface
        rfid: bool,

        #[cfg(feature = "gpio")]
        #[clap(long, default_value_t = false)]
        /// Enable gpio interface for raspberry pi. Pin 16 (gpio-23) will be high when playing
        gpio: bool,

        #[clap(long)]
        /// Cache audio files in directory [default: Temporary directory]
        audio_cache: Option<PathBuf>,

        #[clap(long, default_value_t = 1)]
        /// Hours before audio cache is cleaned. 0 for disable
        audio_cache_time_to_live: u32,
    },
    /// Persist configurations
    Config {
        #[clap(subcommand)]
        command: ConfigCommands,
    },
    /// Refresh database
    #[clap(name = "refresh")]
    RefreshDatabase,
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Set username.
    #[clap(value_parser)]
    Username { username: String },
    /// Set password. Leave empty to get a password prompt.
    #[clap(value_parser)]
    Password { password: Option<String> },
    /// Set max audio quality.
    #[clap(value_parser)]
    MaxAudioQuality {
        #[clap(value_enum)]
        quality: AudioQuality,
    },
}

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("{error}"))]
    PlayerError { error: String },
    #[snafu(display("{error}"))]
    TerminalError { error: String },
    #[snafu(display("No username found. Set with config or arguments"))]
    UsernameMissing,
    #[snafu(display("No password found. Set with config or arguments"))]
    PasswordMissing,
    #[snafu(display("Error reading error prompt"))]
    PasswordError,
}

impl From<qobuz_player_controls::error::Error> for Error {
    fn from(error: qobuz_player_controls::error::Error) -> Self {
        Error::PlayerError {
            error: error.to_string(),
        }
    }
}

pub async fn run() -> Result<(), Error> {
    let cli = Cli::parse();

    let database = Arc::new(Database::new().await?);

    let verbosity = match &cli.command {
        Some(Commands::Open {
            disable_tui,
            rfid,
            web,
            ..
        }) => {
            if cli.verbosity.is_none() && *disable_tui && !*rfid && *web {
                Some(tracing::Level::INFO)
            } else {
                cli.verbosity
            }
        }
        _ => cli.verbosity,
    };

    tracing_subscriber::fmt()
        .with_max_level(verbosity)
        .with_target(false)
        .compact()
        .init();

    match cli.command.unwrap_or(Commands::Open {
        username: Default::default(),
        password: Default::default(),
        max_audio_quality: Default::default(),
        disable_tui: Default::default(),
        #[cfg(target_os = "linux")]
        disable_mpris: Default::default(),
        web: Default::default(),
        web_secret: Default::default(),
        rfid: Default::default(),
        port: Default::default(),
        #[cfg(feature = "gpio")]
        gpio: Default::default(),
        audio_cache: Default::default(),
        audio_cache_time_to_live: Default::default(),
        disable_tui_album_cover: false,
    }) {
        Commands::Open {
            username,
            password,
            max_audio_quality,
            disable_tui,
            #[cfg(target_os = "linux")]
            disable_mpris,
            web,
            web_secret,
            rfid,
            port,
            #[cfg(feature = "gpio")]
            gpio,
            audio_cache,
            audio_cache_time_to_live,
            disable_tui_album_cover,
        } => {
            let database_credentials = database.get_credentials().await?;
            let database_configuration = database.get_configuration().await?;
            let tracklist = database.get_tracklist().await.unwrap_or_default();
            let volume = database.get_volume().await.unwrap_or(1.0);

            let (exit_sender, exit_receiver) = broadcast::channel(5);

            let audio_cache = audio_cache.unwrap_or_else(|| {
                let mut cache_dir = std::env::temp_dir();
                cache_dir.push("qobuz-player-cache");
                cache_dir
            });

            let username = match username {
                Some(username) => username,
                None => database_credentials
                    .username
                    .ok_or(Error::UsernameMissing)?,
            };

            let password = match password {
                Some(p) => p,
                None => database_credentials
                    .password
                    .ok_or(Error::PasswordMissing)?,
            };

            let max_audio_quality = max_audio_quality.unwrap_or_else(|| {
                database_configuration
                    .max_audio_quality
                    .try_into()
                    .expect("This should always convert")
            });

            let client = Arc::new(Client::new(username, password, max_audio_quality));

            let broadcast = Arc::new(NotificationBroadcast::new());
            let mut player = Player::new(
                tracklist,
                client.clone(),
                volume,
                broadcast.clone(),
                audio_cache,
                database.clone(),
            )?;

            let rfid_state = rfid.then(RfidState::default);

            #[cfg(target_os = "linux")]
            if !disable_mpris {
                let position_receiver = player.position();
                let tracklist_receiver = player.tracklist();
                let volume_receiver = player.volume();
                let status_receiver = player.status();
                let controls = player.controls();
                let exit_sender = exit_sender.clone();
                tokio::spawn(async move {
                    if let Err(e) = qobuz_player_mpris::init(
                        position_receiver,
                        tracklist_receiver,
                        volume_receiver,
                        status_receiver,
                        controls,
                        exit_sender,
                    )
                    .await
                    {
                        error_exit(e.into());
                    }
                });
            }

            if web {
                let position_receiver = player.position();
                let tracklist_receiver = player.tracklist();
                let volume_receiver = player.volume();
                let status_receiver = player.status();
                let controls = player.controls();
                let rfid_state = rfid_state.clone();
                let broadcast = broadcast.clone();
                let client = client.clone();
                let database = database.clone();
                let exit_sender = exit_sender.clone();

                tokio::spawn(async move {
                    if let Err(e) = qobuz_player_web::init(
                        controls,
                        position_receiver,
                        tracklist_receiver,
                        volume_receiver,
                        status_receiver,
                        port,
                        web_secret,
                        rfid_state,
                        broadcast,
                        client,
                        database,
                        exit_sender,
                    )
                    .await
                    {
                        error_exit(e.into());
                    }
                });
            }

            #[cfg(feature = "gpio")]
            if gpio {
                let status_receiver = player.status();
                tokio::spawn(async move {
                    if let Err(e) = qobuz_player_gpio::init(status_receiver).await {
                        error_exit(e.into());
                    }
                });
            }

            if let Some(rfid_state) = rfid_state {
                let tracklist_receiver = player.tracklist();
                let controls = player.controls();
                let database = database.clone();
                let broadcast = broadcast.clone();
                tokio::spawn(async move {
                    if let Err(e) = qobuz_player_rfid::init(
                        rfid_state,
                        tracklist_receiver,
                        controls,
                        database,
                        broadcast,
                    )
                    .await
                    {
                        error_exit(e.into());
                    }
                });
            } else if !disable_tui {
                let position_receiver = player.position();
                let tracklist_receiver = player.tracklist();
                let status_receiver = player.status();
                let controls = player.controls();
                let client = client.clone();
                let broadcast = broadcast.clone();
                let database = database.clone();
                tokio::spawn(async move {
                    if let Err(e) = qobuz_player_tui::init(
                        client,
                        broadcast,
                        controls,
                        position_receiver,
                        tracklist_receiver,
                        status_receiver,
                        exit_sender,
                        database,
                        disable_tui_album_cover,
                    )
                    .await
                    {
                        error_exit(e.into());
                    };
                });
            };

            let database_for_cleanup = database.clone();
            let database_for_monitoring = database.clone();
            let broadcast_for_monitoring = broadcast.clone();

            if audio_cache_time_to_live != 0 {
                let clean_up_schedule = every(1).hour().perform(move || {
                    let database = database_for_cleanup.clone();
                    async move {
                        if let Ok(deleted_paths) = database
                            .clean_up_cache_entries(time::Duration::hours(
                                audio_cache_time_to_live.into(),
                            ))
                            .await
                        {
                            for path in deleted_paths {
                                _ = tokio::fs::remove_file(path.as_path()).await;
                            }
                        };
                    }
                });

                tokio::spawn(clean_up_schedule);
            }

            let controls_for_monitoring = player.controls();
            tokio::spawn(async move {
                monitor_audio_devices(controls_for_monitoring, database_for_monitoring, broadcast_for_monitoring).await;
            });

            player.player_loop(exit_receiver).await?;
            Ok(())
        }
        Commands::Config { command } => match command {
            ConfigCommands::Username { username } => {
                database.set_username(username).await?;
                println!("Username saved.");
                Ok(())
            }
            ConfigCommands::Password { password } => {
                let password = match password {
                    Some(password) => password,
                    None => {
                        print!("Password: ");
                        stdout().flush().or(Err(Error::PasswordError))?;
                        stdin()
                            .lines()
                            .next()
                            .expect("encountered EOF")
                            .or(Err(Error::PasswordError))?
                    }
                };
                database.set_password(password).await?;
                println!("Password saved.");
                Ok(())
            }
            ConfigCommands::MaxAudioQuality { quality } => {
                database.set_max_audio_quality(quality).await?;
                println!("Max audio quality saved.");
                Ok(())
            }
        },
        Commands::RefreshDatabase => {
            database.refresh_database().await?;
            println!("Database refreshed successfully.");
            Ok(())
        }
    }
}

async fn monitor_audio_devices(
    controls: qobuz_player_controls::controls::Controls,
    database: Arc<Database>,
    broadcast: Arc<NotificationBroadcast>,
) {
    tracing::info!("Starting audio device monitoring");
    let mut last_device_count = 0;
    let mut last_selected_device: Option<String> = None;
    let mut last_default_device: Option<String> = None;
    
    loop {
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        
        match list_audio_devices() {
            Ok(devices) => {
                let current_count = devices.len();
                let current_selected = database.get_configuration().await
                    .ok()
                    .and_then(|c| c.audio_device_name);
                
                let current_default = get_default_device_name().unwrap_or(None);
                    
                if current_count != last_device_count {
                    tracing::info!("Audio device count changed: {} -> {}", last_device_count, current_count);
                    last_device_count = current_count;
                    
                    broadcast.send(Notification::Info(
                        "Audio device list updated.".to_string()
                    ));
                    
                    if let Some(selected) = &current_selected {
                        let device_exists = devices.iter().any(|d| &d.name == selected);
                        if !device_exists {
                            tracing::warn!("Selected audio device '{}' no longer available", selected);
                            broadcast.send(Notification::Warning(
                                format!("Audio device '{}' was removed. Using default device.", selected)
                            ));
                            if let Err(e) = database.set_audio_device(None).await {
                                tracing::error!("Failed to reset audio device: {}", e);
                            } else {
                                controls.set_audio_device(None);
                            }
                        }
                    }
                }
                
                if current_selected.is_none() {
                    if current_default != last_default_device {
                        if let (Some(old_default), Some(new_default)) = (&last_default_device, &current_default) {
                            if old_default != new_default {
                                tracing::info!("System default device changed: '{}' -> '{}'", old_default, new_default);
                                broadcast.send(Notification::Info(
                                    format!("Default audio device changed to '{}'.", new_default)
                                ));
                                controls.set_audio_device(None);
                            }
                        } else if current_default.is_some() && last_default_device.is_none() {
                            if let Some(new_default) = &current_default {
                                tracing::info!("Default audio device appeared: '{}'", new_default);
                                broadcast.send(Notification::Info(
                                    format!("Default audio device is now '{}'.", new_default)
                                ));
                                controls.set_audio_device(None);
                            }
                        } else if last_default_device.is_some() && current_default.is_none() {
                            tracing::warn!("Default audio device disappeared");
                            broadcast.send(Notification::Warning(
                                "Default audio device was removed.".to_string()
                            ));
                            controls.set_audio_device(None);
                        }
                        last_default_device = current_default.clone();
                    }
                } else {
                    last_default_device = current_default.clone();
                }
                
                if current_selected.is_none() && current_count != last_device_count {
                    tracing::info!("Device list changed while 'Default' is selected, rechecking default device");
                    controls.set_audio_device(None);
                }
                
                if current_selected != last_selected_device {
                    tracing::info!("Selected audio device changed externally: {:?} -> {:?}", 
                        last_selected_device, current_selected);
                    last_selected_device = current_selected.clone();
                }
            }
            Err(e) => {
                tracing::error!("Failed to list audio devices: {}", e);
            }
        }
    }
}

fn error_exit(error: Error) {
    eprintln!("{error}");
    std::process::exit(1);
}

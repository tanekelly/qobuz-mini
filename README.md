# qobuz-mini

Powered by [Qobuz](https://www.qobuz.com). Requires a paid subscription. This does not allow you to listen for free.

This project is a fork of [qobuz-player](https://github.com/SofusA/qobuz-player) with added features for musicians and to integrate with my stream deck plugin

## Web UI
![Web UI Screenshot](/assets/qobuz-player-webui.png)

## Terminal UI
![TUI Screenshot](/assets/qobuz-player.png)

Currently only being developed for windows.

## Todo

### Currently Planned Features
- Username & password authentication on Web UI
- UI based on Qobuz design
- Audio drivers list

### Future Planned Features
- Stream Deck integration
- Change playback speed & pitch
- Create & save timestamps
- Create & save loop sections
- Audio equalizer


## Installation
### Fonts
The terminal ui needs a [nerdfont](https://www.nerdfonts.com/) to display icons for explicit and hi-resolution.

### Download Release

Download the tar.gz file for your supported OS from the releases page, extract the file and execute `qobuz-player` or copy it to your `$PATH`.

### Installation with cargo
```
cargo install --git https://github.com/SofusA/qobuz-player
```

### Build from source

Dependencies: `llvm` (for pitch and time)

Linux dependencies: `alsa-sys-devel`, `just`.
```
cargo build
```

## Development
1. Setup sqlx: `just create-env-file`. Only needed once. 
2. Init sqlite database: `init-database`.
3. For webui development in `qobuz-player-web`:
  - `npm i`. Install npm dependencies. 
  - `npm run watch`. Watch for style changes. 

## Get started

Run `qobuz-player --help` or `qobuz-player <subcommand> --help` to see all available options.

To get started:

```shell
qobuz-player config username {USERNAME}
qobuz-player config password {PASSWORD}
# or to get prompted for the password
qobuz-player config password

# open tui player
qobuz-player

# open player with web ui
qobuz-player open --web 

# refresh database
qobuz-player refresh
```

## Web UI

The player can start an embedded web interface. This is disabled by default and must be started with the `--web` argument. It also listens on `0.0.0.0:9888` by default. Change port with `--port` argument.

Go to `http://localhost:9888` to view the UI.

## Contribution
Feature requests, issues and contributions are very welcome.

## Credits
This codebase is a fork of [qobuz-player](https://github.com/SofusA/qobuz-player) made by [SofusA](https://github.com/SofusA) which is based off [hifi.rs](https://github.com/iamdb/hifi.rs) by [David Benjamin](https://github.com/iamdb)

### Pitch & time stretching
- [Signalsmith Stretch](https://github.com/Signalsmith-Audio/signalsmith-stretch)
- [signalsmith-stretch-rs](https://github.com/colinmarc/signalsmith-stretch-rs)
- [Geraint Luff](https://geraintluff.github.io/jsfx/) - [Audio Developer Conference 2022](https://www.youtube.com/watch?v=fJUmmcGKZMI)

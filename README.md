<div align="center">

<img src="crates/app/ui/icons/axi-icon.svg" width="120" alt="Axi Launcher logo, a smiling axolotl" />

# Axi Launcher

**A lightweight, open-source Minecraft launcher with modpacks, shaders and LAN world sync.**

Install Fabric and NeoForge modpacks from Modrinth and CurseForge in one click, add shaders without the setup,
and keep your worlds and mods in sync between your own computers. Free, fast, and about 50 MB of RAM.

[**Download for Windows and macOS**](https://github.com/neerajsunil/axi-launcher/releases/latest)

</div>

<p align="center">
  <img src="docs/screenshots/home.webp" alt="Axi Launcher home screen with a one-click button to jump back into the last played Minecraft instance" />
</p>

## Why Axi

Most Minecraft launchers are either bare-bones or heavy and cluttered. Axi tries to be neither.

- **Light.** Around 50 MB of RAM while idle, so your game gets the memory, not the launcher.
- **Simple.** One clean interface. Pick a version or a modpack, press Play.
- **Mods without the mess.** Search, install and update mods, modpacks, resource packs and shaders inside the launcher.
- **Your worlds, on every computer.** Play on your desktop, pick up where you left off on your laptop.

## Features

### Modpacks and mods
- Browse and install **Modrinth** and **CurseForge** modpacks in one click
- Import modpack files you already have (`.mrpack` and CurseForge `.zip`)
- Install and update **mods, resource packs and shader packs**, with update checks for everything installed
- Only shows versions that work with your instance's Minecraft version and mod loader

### Mod loaders and versions
- **Vanilla, Fabric and NeoForge**, for every Minecraft release from 1.13 onward, plus snapshots
- Change an instance's game version or mod loader at any time

### Shaders made easy
- Axi installs **Iris** for you, so shader packs work straight away

### LAN sync between your computers
- Turn on sync for an instance and it stays the same on your other computers running Axi
- Syncs **mods, resource packs, shader packs, worlds and your server list**
- Pulls the latest changes when you press Play, and shares yours when you close the game
- Works over your home network. No cloud account, and your worlds never leave your network
- If the same world was played on two computers, both copies are kept

### The rest
- **Automatic Java**: the right Java version is downloaded for each game version
- **Microsoft accounts** and offline accounts, with your skin's face shown
- A tidy **library** with search, sorting, groups and custom icons
- Play time tracking and a "jump back in" button for your last instance
- **Automatic updates** in the background, installed on restart
- Native app for **Windows** and **macOS (Apple Silicon)**, built in Rust

## Screenshots

<p align="center">
  <img src="docs/screenshots/library.webp" alt="Axi Launcher library showing Minecraft instances with Fabric and NeoForge" />
</p>

<p align="center">
  <img src="docs/screenshots/discover.webp" alt="Browsing Minecraft modpacks from Modrinth and CurseForge in Axi Launcher" />
</p>

## Getting started

1. [Download Axi](https://github.com/neerajsunil/axi-launcher/releases/latest) for Windows or macOS.
2. Sign in with your Microsoft account.
3. Make a new instance, or pick a modpack from **Discover**.
4. Press **Play**.

**macOS:** the app isn't notarized yet, so the first time, right-click it and choose **Open**.

## FAQ

**How do I sync Minecraft worlds between two computers?**
Install Axi on both, sign in with the same account, and turn on **LAN sync** in the instance's Overview tab.
When both computers are on the same network, the instance shows up on the other one under **On your network** in the Library.
After that, every time you press Play, Axi pulls in what changed.

**Does Axi work with Fabric and NeoForge modpacks?**
Yes. Fabric and NeoForge packs from Modrinth and CurseForge install in one click. Forge and Quilt packs aren't supported yet.

**How do I install shaders?**
Open an instance, go to **Shaders**, and press **Install Iris**. After that, any shader pack you add just works.

**Is Axi free and open source?**
Yes. It's MIT licensed.

**Does it run on Apple Silicon Macs?**
Yes, natively. Minecraft 1.19 and newer runs natively too; 1.13 to 1.18 runs through Rosetta 2 because those versions have no Apple Silicon graphics libraries.

**How much memory does it use?**
About 50 MB while idle, so it can stay open while you play.

## Building from source

You'll need [Rust](https://rustup.rs). Then:

```bash
cargo run --release -p axi-app
```

Microsoft sign-in and CurseForge browsing need keys, which are kept out of this repository.
Provide them as environment variables when you build (or under `[env]` in your own `~/.cargo/config.toml`).
They're stored scrambled in the binary.

| Variable | What it's for |
| --- | --- |
| `AXI_MSA_CLIENT_ID` | Azure app client ID, for Microsoft sign-in |
| `AXI_CURSEFORGE_API_KEY` | CurseForge API key, for browsing CurseForge |

Both can also be entered in Axi's **Settings**.

### Release builds

| Platform | Command | Output |
| --- | --- | --- |
| Windows 64-bit | `cargo build --release -p axi-app --target x86_64-pc-windows-msvc` | `axi-launcher.exe` (no VC++ redistributable needed) |
| macOS Apple Silicon | `packaging/macos/bundle.sh` (on a Mac) | `Axi Launcher.app` and a `.dmg` |

GitHub Actions builds both on every push, and drafts a release when a `v*` tag is pushed.
Add `AXI_MSA_CLIENT_ID` and `AXI_CURSEFORGE_API_KEY` as repository secrets to build the keys in.

## License

[MIT](LICENSE)

<div align="center">

*Axi Launcher is not affiliated with Mojang or Microsoft. Minecraft is a trademark of Mojang AB.*

</div>

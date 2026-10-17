# WhatsMine

**Chat without servers, accounts or phone numbers. What's yours stays yours.**

WhatsMine is a free, open-source 1:1 messenger with no company in the middle:

- **No account and no phone number.** On first launch your device creates a key that only you have. Your key is your address: share it (copy or QR code) with people you want to talk to.
- **No servers.** Your device connects straight to theirs, end-to-end encrypted (QUIC + TLS 1.3 via [iroh](https://iroh.computer)). Nothing is stored anywhere except on your own devices.
- **You decide who can reach you.** Someone who adds your key sends a request, and nothing gets through until you accept. Blocking is silent.
- **Nobody can take it away.** No account can be banned and no database can leak, be sold or be subpoenaed. If you delete the app, your data is gone.

It runs on Windows and Android from one Rust codebase (the code is portable to macOS, Linux and iOS, which haven't been tried). Device features (file picker, camera, microphone, open and save) come from [mobile-sentinel](https://github.com/fidderr/mobile-sentinel).

## Features

- 1:1 chats with history saved on your device
- **Photos, videos, voice messages and any file**, shown inline (images and GIFs, video and audio players) or as a file card. Take a photo, record a video or a voice note right in the chat, or pick from the gallery. Files up to 2 GB by default; each person sets the largest file they accept, and bigger ones are declined with a message to the sender. Transfers resume where they stopped and are checked with a SHA-256 hash.
- **Voice and video calls**, peer to peer like everything else: audio (Opus) goes as datagrams and video (VP8) as one short stream per frame, straight between the two devices, encrypted by the same connection. No call servers. Mute, camera on/off, and a call screen with ringing, accept and decline. Only contacts who are online can be called.
- **Profile pictures** that your contacts see
- Contact requests: accept, decline or block
- Sent ✓, delivered ✓✓ and read ✓✓ (blue) ticks
- Online, last seen and typing indicators; each can be switched off, and then you don't see other people's either
- Messages you write while the other person is offline are queued and sent automatically when you're both online
- Nicknames, a safety key to compare in person, clear chat, delete contact
- Dark, light or follow-the-device theme
- English, Nederlands, Español, Français, Deutsch
- "Delete everything" wipes your identity and all chats

## How it works

```
 your device                                          their device
┌──────────────┐   direct QUIC, end-to-end encrypted ┌──────────────┐
│ WhatsMine    │◄───────────────────────────────────►│ WhatsMine    │
│ key + SQLite │        (hole-punched through        │ key + SQLite │
└──────────────┘         home routers / 5G)          └──────────────┘
        │  "where is key X right now?"                       │
        └──────► DHT / DNS / local network ◄─────────────────┘
```

- **Identity:** an Ed25519 keypair generated on the device. The public key (52 characters, z-base-32) is your address. It's unique by math, so nothing has to register it.
- **Finding each other:** a device publishes "key X is reachable here", signed with its key. It publishes to the BitTorrent Mainline DHT (millions of nodes, owned by no one), to n0's public DNS, and to the local network (mDNS). Peers look you up by key.
- **Connecting:** iroh hole-punches a direct connection. About 10% of networks can't be punched through. Then traffic goes through a relay, which only ever sees encrypted bytes.
- **Protocol:** small length-prefixed messages on one QUIC stream. A message counts as delivered only when the other device acknowledges it, and anything unacknowledged is re-sent on the next connection. Duplicates are ignored, so nothing is lost or doubled.
- **Storage:** one SQLite file in the app's private folder.

## Honest limits (v1)

- **Both devices must be online at the same time** for a message to move. Messages wait on your device until then. There's no offline mailbox yet.
- **No background delivery or notifications.** The app has to be open.
- **One device per identity.** Linking a phone and a laptop is planned.
- **Relays and discovery defaults:** the fallback relays and the DNS discovery service are run by n0. They can't read messages, and the DHT and local discovery work without them. Running your own relay will be supported.
- **Your contacts see your IP address**, because you connect to them directly.
- **The local database isn't encrypted at rest.** It relies on the OS's app sandbox.
- No groups yet. Large files need both of you online (they resume if the connection drops).

## Build

Requirements: Rust (stable), [`dx`](https://dioxuslabs.com/learn/0.7/getting_started) 0.7 (`cargo install dioxus-cli`), a checkout of [mobile-sentinel](https://github.com/fidderr/mobile-sentinel) next to this repo (until 0.2.2 is published), and for Android, the Android SDK, NDK 28 and JDK 17.

```sh
./build.sh                    # desktop app (release)
./build.sh android install    # Android debug APK, installed over adb (keeps chats on update)
./build.sh cli                # headless peer for testing
./build.sh test               # fmt, clippy, tests
```

To run two instances on one computer, give each its own profile:

```sh
WHATSMINE_PROFILE=alice target/dx/whatsmine/release/windows/app/whatsmine.exe
WHATSMINE_PROFILE=bob   target/dx/whatsmine/release/windows/app/whatsmine.exe
```

## Project layout

| Folder | What |
|---|---|
| `core/` | `whatsmine-core`: identity, iroh networking, protocol, SQLite storage. No UI. Integration tests run two real peers over loopback. |
| `apps/whatsmine/` | `whatsmine`: the Dioxus UI. Design and patterns are shared with [AlarmFree](https://github.com/fidderr/alarmfree). |
| `cli/` | `whatsmine-cli`: a terminal peer on the same core, for testing and scripting. |

## License

MIT

# WhatsMine

**Chat without servers, accounts or phone numbers. What's yours stays yours.**

WhatsMine is a free, open-source messenger with no company in the middle:

- **No account and no phone number.** Your device makes a key only you have, from 12 words that bring you back if you lose it.
- **Nothing to run.** Phones and PCs find each other through the BitTorrent DHT, which nobody owns, and connect directly. You and the people you talk to set up nothing.
- **Who you are is not where you are.** Strangers only get a link, a name code or a QR code, and those only let them leave a sealed request. They never reach your phone and never learn its address.
- **Being bothered?** One button makes all your links stop working. Your contacts keep talking to you and notice nothing.
- **Encrypted end to end,** per device, with Olm. Names, messages and keys on disk are sealed with a key the system holds (Android Keystore, Windows DPAPI).

It runs on Android 12 and newer (64-bit) and on Windows, from one Rust codebase.

## What it does

- **Chats:** replies, reactions, edits, taking back for everyone, mentions, bold, italic, code and spoilers, pinned messages, select many, search, export
- **Rich messages:** polls, shared lists, plans with who is coming, places that open in your maps app, contact cards
- **Files:** photos, videos, voice messages with the real sound shown and a speed button, any file. Original quality, no size limit other than what the receiver allows
- **Calls:** voice and video, one to one and in groups
- **Groups:** up to 64 members, owner and admins, who may send, add and edit, slow mode, invite links with or without approval, and a group goes on when its owner leaves. Any member can pass on what someone missed
- **Your devices:** link a phone and a PC. Everything shows up on both, also when the other was off
- **Never silently stuck:** a message says where it is, why it waits and when the next try is. Past the time you set it says it did not get there, and can be tried again
- **Battery:** nothing is held open on a sleeping phone on mobile data. Short texts wait sealed in the DHT
- **Staying on top of things:** folders by rule, labels, archive, snooze, hidden chats, send later, send without sound, reminders, remind me if nobody answers, your own list, notes to self, quick replies
- **Your look:** themes including pure black and colorblind, accent colours, text size, spacing, wallpapers and bubble colour per chat, flat layout
- **Privacy:** app lock, disappearing messages, per-person name and visibility, screenshot block, incognito keyboard, safety words, a filter and a quiet mode for requests
- **Coming back:** recovery words, and encrypted backups only those words open
- English, Nederlands, Español, Français, Deutsch

## How it works

```
 Phone or PC                                       Phone or PC
┌──────────────┐   direct QUIC, sealed end to end ┌──────────────┐
│ WhatsMine    │◄────────────────────────────────►│ WhatsMine    │
└──────┬───────┘                                  └──────┬───────┘
       │      sealed "where am I", requests, knocks      │
       └───────────────►  BitTorrent DHT  ◄──────────────┘
```

- **Person key:** who you are. Contacts know it. A leaked one says nothing about where you are.
- **Network key:** your address on a connection. Rotates weekly and on reset.
- **Location secret:** only accepted contacts have it. With it they find your sealed, signed address in the DHT, under a key that changes daily.
- **Invites:** a link, QR code or name code like `roy-7kq4x2`. Each lets someone leave a sealed request in a few slots of the DHT, read when you look. A flood costs you nothing.
- **A connection:** each side signs over this very connection who it is, then everything is sealed with Olm. Whoever is no contact is refused right after that.
- **Delivery:** each message has its own retry state on disk: at once when something changes, then 30 seconds, 2, 5, 15, 30 minutes, then hourly, for as long as you set.

## Honest limits

- **A sleeping phone on mobile data is not instant by default.** Messages arrive within minutes. A setting makes it instant for more battery.
- **Contacts you accepted see your IP address** on a direct connection. Nobody else does.
- **Public relays run by n0** help when a direct connection is impossible. They carry sealed traffic and see IP addresses, n0 offers them without a guarantee, and one switch turns them off.
- **Links** go through a static page (`site/`) because chat apps do not make `whatsmine://` clickable. The part after the # never reaches any server.
- **Two devices per person.** A lost linked device holds your key until it is removed.
- **Files stay on the device that has them.** Files of others can be fetched again from whoever sent them.
- **What a copy of the app's folder shows.** Names, messages and keys cannot be read from it. Contact keys and times can, and so can photos and files, which lie as they are in the app's private folder.
- **Photos lose their location, other files do not.** JPEG, PNG and WebP go out without metadata. Videos, HEIC and other files go exactly as they are.
- **Not audited.** Nothing here has had an outside security audit yet.

## Build

Requirements: Rust (stable), [`dx`](https://dioxuslabs.com/learn/0.7/getting_started) 0.7, a checkout of [mobile-sentinel](https://github.com/fidderr/mobile-sentinel) next to this repo, and for Android its SDK and NDK.

```sh
./build.sh desktop            # the Windows app
./build.sh android install    # Android debug APK, installed over adb
./build.sh aab                # what goes to Play; sign it with ./sign-aab.sh
./build.sh cli                # a terminal peer for testing
./build.sh test               # fmt, clippy, tests
```

## Tests

| What | How |
|---|---|
| Unit and two-peer tests | `cargo test --workspace`. Real cores over loopback QUIC, with kills and restarts |
| The real internet | `cargo test -p whatsmine-core --test real_network -- --ignored` |
| Every screen, with pictures | `PLATFORM=windows bash devtest/tour.sh`, `PLATFORM=android bash devtest/tour.sh`. Variants with `THEME=light LANG_=nl` |
| The real Android signals | `bash devtest/connection.sh`, on an emulator |
| Networks measured | `tools/netlab` |

Test windows stay off-screen and never take the focus, and test builds have their own app id and data.

## Project layout

| Folder | What |
|---|---|
| `core/` | Identity, connections, encryption, delivery, groups, linked devices, storage. No UI |
| `apps/whatsmine/` | The Dioxus UI for Android and Windows |
| `cli/` | `whatsmine-cli`, a terminal peer on the same core |
| `devtest/` | The app scenarios, one library for Android and Windows |
| `tools/netlab/` | Measures NAT type, IPv6, relays, DHT and connecting |
| `site/` | The static page behind `https` links |

## License

MIT

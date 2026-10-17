//! Headless WhatsMine peer. Same core as the app, driven from the terminal.
//!
//!   whatsmine-cli [--profile NAME] [--name DISPLAY_NAME]
//!
//! Lines starting with `/` are commands (`/help`); anything else is sent to
//! the current chat.

use tokio::io::{AsyncBufReadExt, BufReader};

/// Whose call media is echoed back: empty = everyone, "off" = nobody,
/// otherwise a key prefix. (Two echoing peers in one group call would
/// otherwise feed each other forever.)
static ECHO: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
use whatsmine_core::{Core, CoreEvent, MessageStatus, NetMode, NetStatus, paths};

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let profile = arg("--profile");
    let dir = paths::data_dir(profile.as_deref())?;
    let core = Core::start(&dir, NetMode::Internet)?;

    let mut settings = core.settings();
    if let Some(name) = arg("--name") {
        settings.display_name = name;
        settings.onboarded = true;
        core.save_settings(&settings)?;
    }

    println!(
        "whatsmine-cli  profile: {}",
        profile.as_deref().unwrap_or("(default)")
    );
    println!("data: {}", dir.display());
    println!("name: {}", core.settings().display_name);
    println!("your key: {}", core.my_key());
    println!("/help for commands");

    // Calls: whatever media arrives is echoed back, and counted, so a phone
    // calling this peer hears and sees itself and the test can tell that
    // every stage of the pipeline worked.
    if let Some(mut media) = core.take_call_media() {
        let echo = core.clone();
        tokio::spawn(async move {
            use whatsmine_core::calls::MediaKind;
            let mut counts: std::collections::BTreeMap<String, (u64, u64)> =
                std::collections::BTreeMap::new();
            let mut last = std::time::Instant::now();
            while let Some(m) = media.recv().await {
                let entry = counts.entry(m.from.chars().take(8).collect()).or_default();
                let echoed = {
                    let filter = ECHO.lock().unwrap_or_else(|p| p.into_inner());
                    filter.is_empty() || (*filter != "off" && m.from.starts_with(filter.as_str()))
                };
                match m.kind {
                    MediaKind::Audio => {
                        entry.0 += 1;
                        if echoed {
                            echo.call_audio(m.ts, m.data);
                        }
                    }
                    MediaKind::Video => {
                        entry.1 += 1;
                        if echoed {
                            echo.call_video(m.ts, m.key, m.data);
                        }
                    }
                }
                if last.elapsed() >= std::time::Duration::from_secs(2) {
                    let line: Vec<String> = counts
                        .iter()
                        .map(|(who, (a, v))| format!("{who}: {a} audio {v} video"))
                        .collect();
                    println!("  · call media in: {}", line.join(" | "));
                    last = std::time::Instant::now();
                }
            }
        });
    }

    let mut events = core.subscribe();
    let ev_core = core.clone();
    tokio::spawn(async move {
        while let Ok(ev) = events.recv().await {
            print_event(&ev_core, &ev);
        }
    });

    let mut current: Option<String> = core.chats().first().map(|c| c.contact.id.clone());
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(cmd) = line.strip_prefix('/') {
            let mut parts = cmd.splitn(3, ' ');
            let name = parts.next().unwrap_or_default();
            let a1 = parts.next().unwrap_or_default();
            let a2 = parts.next().unwrap_or_default();
            if name == "quit" {
                break;
            }
            if let Err(e) = run(&core, &mut current, name, a1, a2) {
                println!("! {e}");
            }
        } else {
            match &current {
                Some(id) => {
                    if let Err(e) = core.send(id, line) {
                        println!("! {e}");
                    }
                }
                None => println!("! no chat selected: /to <name|key> <text> or /open <name>"),
            }
        }
    }
    core.shutdown();
    Ok(())
}

/// Resolves a nickname, name or key prefix to a contact id.
fn resolve(core: &Core, r: &str) -> Option<String> {
    let r = r.to_lowercase();
    let all = core
        .chats()
        .into_iter()
        .map(|c| c.contact)
        .chain(core.requests());
    let mut matches: Vec<_> = all
        .filter(|c| c.display_name().to_lowercase() == r || c.id.starts_with(&r))
        .map(|c| c.id)
        .collect();
    matches.dedup();
    (matches.len() == 1).then(|| matches.remove(0))
}

fn run(
    core: &Core,
    current: &mut Option<String>,
    name: &str,
    a1: &str,
    a2: &str,
) -> anyhow::Result<()> {
    let need = |r: &str| resolve(core, r).ok_or_else(|| anyhow::anyhow!("unknown contact '{r}'"));
    match name {
        "whoami" => println!("{} — {}", core.settings().display_name, core.my_key()),
        "name" => {
            let mut s = core.settings();
            s.display_name = format!("{a1} {a2}").trim().to_string();
            core.save_settings(&s)?;
            println!("· name set to {}", s.display_name);
        }
        "add" => {
            let c = core.add_contact(a1, (!a2.is_empty()).then_some(a2))?;
            *current = Some(c.id.clone());
            println!("· request sent to {} — they must accept", c.display_name());
        }
        "accept" => {
            let id = need(a1)?;
            core.accept(&id)?;
            if !a2.is_empty() {
                core.rename(&id, a2);
            }
            *current = Some(id);
            println!("· accepted");
        }
        "decline" => core.decline(&need(a1)?)?,
        "block" => core.block(&need(a1)?)?,
        "rename" => core.rename(&need(a1)?, a2),
        "open" => {
            let id = need(a1)?;
            for m in core.messages(&id, None, 30) {
                print_message(core, &m);
            }
            core.mark_read(&id);
            *current = Some(id);
        }
        "to" => {
            let id = need(a1)?;
            core.send(&id, a2)?;
            *current = Some(id);
        }
        "read" => {
            if let Some(id) = current {
                core.mark_read(id);
            }
        }
        "file" => {
            let id = need(a1)?;
            let path = std::path::Path::new(a2);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mime = guess_mime(&name);
            let m = core.send_file(&id, path, &name, mime, "", None)?;
            println!(
                "· sending {} ({} bytes)",
                name,
                m.attachment.map(|a| a.size).unwrap_or(0)
            );
            *current = Some(id);
        }
        "avatar" => {
            if a1.is_empty() {
                core.clear_avatar()?;
            } else {
                core.set_avatar(std::path::Path::new(&format!("{a1} {a2}").trim()))?;
            }
        }
        "call" => {
            let id = need(a1)?;
            core.call_invite(&id, a2 == "video")?;
            println!("· calling…");
        }
        "newgroup" => {
            let members: Vec<String> = a2
                .split_whitespace()
                .map(&need)
                .collect::<anyhow::Result<_>>()?;
            let g = core.create_group(a1, &members)?;
            *current = Some(g.id.clone());
            println!("· group {} created ({})", g.display_name(), g.id);
        }
        "gadd" => {
            core.group_add(&need(a1)?, &need(a2)?)?;
        }
        "gremove" => {
            core.group_remove(&need(a1)?, &need(a2)?)?;
        }
        "grename" => {
            core.group_rename(&need(a1)?, a2)?;
        }
        "gleave" => core.leave_group(&need(a1)?)?,
        "gdelete" => core.delete_contact(&need(a1)?)?,
        "gjoin" => core.call_join(&need(a1)?),
        "ginfo" => {
            let id = need(a1)?;
            let info = core
                .group_info(&id)
                .ok_or_else(|| anyhow::anyhow!("not a group"))?;
            println!(
                "· group {} active:{} admin:{}",
                info.name, info.active, info.i_am_admin
            );
            for m in info.members {
                println!(
                    "    {}{}{} {}",
                    m.name,
                    if m.is_me { " (me)" } else { "" },
                    if m.is_admin { " (admin)" } else { "" },
                    if m.online { "online" } else { "" }
                );
            }
        }
        "echo" => {
            let f = if a1 == "all" {
                String::new()
            } else {
                a1.to_string()
            };
            *ECHO.lock().unwrap_or_else(|p| p.into_inner()) = f;
            println!("· echo: {}", if a1.is_empty() { "all" } else { a1 });
        }
        "answer" => core.call_answer(true),
        "refuse" => core.call_answer(false),
        "hangup" => core.call_end(),
        "chats" => {
            for c in core.chats() {
                let online = if core.is_online(&c.contact.id) {
                    "online"
                } else {
                    ""
                };
                println!(
                    "  {:<20} {:?} {} unread:{}",
                    c.contact.display_name(),
                    c.contact.state,
                    online,
                    c.unread
                );
            }
            for r in core.requests() {
                println!(
                    "  {:<20} wants to connect ({})",
                    r.display_name(),
                    &r.id[..8]
                );
            }
        }
        "help" => println!(
            "/whoami  /name <name>  /add <key> [nick]  /accept <who> [nick]  /decline <who>\n\
             /block <who>  /rename <who> <nick>  /open <who>  /to <who> <text>  /read\n\
             /chats  /quit  /call <who|group> [video]  /echo <key prefix|off|all>  /answer  /refuse  /hangup\n\
             /newgroup <name> <who>...  /gadd <group> <who>  /gremove <group> <who>  /grename <group> <name>\n\
             /gleave <group>  /gdelete <group>  /ginfo <group>  /gjoin <group>  (groups work with /to and /file too)       <who> = nickname, name or key prefix; plain text → current chat"
        ),
        _ => anyhow::bail!("unknown command, try /help"),
    }
    Ok(())
}

fn guess_mime(name: &str) -> &'static str {
    match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn label(core: &Core, id: &str) -> String {
    core.contact(id)
        .map(|c| c.display_name())
        .unwrap_or_else(|| id.chars().take(8).collect())
}

fn print_message(core: &Core, m: &whatsmine_core::Message) {
    let who = if m.outgoing {
        "me".to_string()
    } else if let Some(sender) = &m.sender {
        format!("{} in {}", label(core, sender), label(core, &m.contact_id))
    } else {
        label(core, &m.contact_id)
    };
    let tick = if m.outgoing { ticks(m.status) } else { "" };
    let what = match &m.attachment {
        Some(a) => format!(
            "[file {} {} bytes {:?}] {}",
            a.name, a.size, a.state, m.body
        ),
        None => m.body.clone(),
    };
    println!("[{}] {who}: {what} {tick}", m.id.get(..8).unwrap_or(""));
}

fn ticks(s: MessageStatus) -> &'static str {
    match s {
        MessageStatus::Queued => "(queued)",
        MessageStatus::Sent => "✓",
        MessageStatus::Delivered => "✓✓",
        MessageStatus::Read => "✓✓ read",
    }
}

fn print_event(core: &Core, ev: &CoreEvent) {
    match ev {
        CoreEvent::MessageAdded(m) if !m.outgoing => print_message(core, m),
        CoreEvent::MessageUpdated(m) => {
            if let Some(a) = &m.attachment {
                println!(
                    "  · file {} {:?} {}",
                    a.name,
                    a.state,
                    a.path.as_deref().unwrap_or("")
                );
            }
        }
        CoreEvent::MessageStatus {
            message_id, status, ..
        } => println!("  · {} {}", &message_id[..8], ticks(*status)),
        CoreEvent::Presence {
            contact_id, online, ..
        } => println!(
            "· {} is {}",
            label(core, contact_id),
            if *online { "online" } else { "offline" }
        ),
        CoreEvent::Typing { contact_id } => println!("· {} is typing…", label(core, contact_id)),
        CoreEvent::RequestReceived { contact_id, name } => println!(
            "? {} ({}) wants to connect — /accept {}",
            if name.is_empty() { "someone" } else { name },
            &contact_id[..8],
            &contact_id[..8]
        ),
        CoreEvent::Accepted { contact_id } => {
            println!("· {} accepted — you can chat", label(core, contact_id))
        }
        CoreEvent::Call(e) => {
            use whatsmine_core::calls::CallEvent;
            match e {
                CallEvent::Participants { peers, .. } => println!(
                    "☎ in the call: {}",
                    peers
                        .iter()
                        .map(|p| label(core, p))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                CallEvent::GroupCall {
                    group_id, active, ..
                } => println!(
                    "☎ group call in {} is {}",
                    label(core, group_id),
                    if *active { "going on (/gjoin)" } else { "over" }
                ),
                CallEvent::Incoming {
                    contact_id, video, ..
                } => println!(
                    "☎ incoming {} call from {} — /answer or /refuse",
                    if *video { "video" } else { "voice" },
                    label(core, contact_id)
                ),
                CallEvent::Connected { .. } => println!("☎ call connected"),
                CallEvent::PeerVideo { on, from, .. } => println!(
                    "☎ {}'s camera is {}",
                    label(core, from),
                    if *on { "on" } else { "off" }
                ),
                CallEvent::Ended {
                    reason,
                    duration_ms,
                    ..
                } => println!("☎ call ended: {reason:?} after {} s", duration_ms / 1000),
            }
        }
        CoreEvent::Net(s) => println!(
            "· network: {}",
            match s {
                NetStatus::Connecting => "connecting…",
                NetStatus::Online => "online",
                NetStatus::Offline => "offline",
            }
        ),
        _ => {}
    }
}

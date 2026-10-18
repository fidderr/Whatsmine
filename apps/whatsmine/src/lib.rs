//! WhatsMine app: the Dioxus UI on top of `whatsmine-core`.

#[cfg(target_os = "android")]
pub mod android;
pub mod app;
pub mod back;
pub mod boot;
pub mod call;
pub mod clipboard;
pub mod components;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod fmt;
pub mod i18n;
pub mod js;
pub mod links;
pub mod media;
pub mod media_server;
pub mod notify;
pub mod platform;
pub mod screens;
pub mod shell;

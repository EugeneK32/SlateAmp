//! MPRIS 2.2 export through the same GIO main loop as GTK.
//! No background D-Bus worker, global keyboard hook or extra dependency is needed.
use crate::{SlateAmp, Repeat, Track};
use adw::prelude::*;
use adw::gtk::{gio, glib};
use glib::variant::ToVariant;
use std::rc::Rc;

const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const BUS_NAME: &str = "org.mpris.MediaPlayer2.slateamp";

fn track_path(serial: u64) -> String { format!("/io/github/eugenek32/SlateAmp/track/{serial}") }

impl SlateAmp {
    fn mpris_current_track(&self) -> Option<Track> {
        self.active_radio.borrow().clone().or_else(|| {
            self.current.get().and_then(|i| self.queue.borrow().get(i).cloned())
        })
    }

    fn mpris_metadata(&self) -> glib::Variant {
        let dict = glib::VariantDict::new(None);
        if let Some(track) = self.mpris_current_track() {
            let id = track_path(self.mpris_serial.get());
            if let Ok(path) = glib::variant::ObjectPath::try_from(id) {
                dict.insert_value("mpris:trackid", &path.to_variant());
            }
            let detail = self.subtitle.text().to_string();
            // For radio the station's name is shown above the stream's ICY tags.
            let (artist, song) = detail.split_once(" — ").unwrap_or(("", ""));
            let title = if song.is_empty() { track.title.clone() } else { song.to_owned() };
            dict.insert_value("xesam:title", &title.to_variant());
            if !artist.is_empty() {
                dict.insert_value("xesam:artist", &vec![artist.to_owned()].to_variant());
            }
            dict.insert_value("xesam:url", &track.uri.to_variant());
            if !track.is_radio {
                if let Some(seconds) = self.audio.duration() {
                    dict.insert_value("mpris:length", &((seconds as i64) * 1_000_000).to_variant());
                }
            }
        }
        dict.end()
    }

    fn mpris_status(&self) -> &'static str {
        if self.playing.get() { "Playing" }
        else if self.mpris_current_track().is_some() { "Paused" }
        else { "Stopped" }
    }

    fn mpris_property(&self, interface: &str, name: &str) -> glib::Variant {
        if interface == ROOT {
            match name {
                "CanQuit" | "CanRaise" => true.to_variant(),
                "HasTrackList" => false.to_variant(),
                "Identity" => "SlateAmp".to_variant(),
                "DesktopEntry" => "io.github.eugenek32.SlateAmp".to_variant(),
                "SupportedUriSchemes" => vec!["file", "http", "https"].to_variant(),
                "SupportedMimeTypes" => vec!["audio/mpeg", "audio/flac", "audio/ogg", "audio/x-wav"].to_variant(),
                _ => "".to_variant(),
            }
        } else {
            let current = self.mpris_current_track();
            let has_track = current.is_some();
            let is_radio = current.as_ref().is_some_and(|t| t.is_radio);
            let queue_available = !self.queue.borrow().is_empty() && !is_radio;
            match name {
                "PlaybackStatus" => self.mpris_status().to_variant(),
                "LoopStatus" => match self.settings.borrow().repeat {
                    Repeat::Off => "None", Repeat::All => "Playlist", Repeat::One => "Track",
                }.to_variant(),
                "Shuffle" => self.settings.borrow().shuffle.to_variant(),
                "Rate" | "MinimumRate" | "MaximumRate" => 1.0f64.to_variant(),
                "Metadata" => self.mpris_metadata(),
                "Position" => ((self.audio.position().unwrap_or(0) as i64) * 1_000_000).to_variant(),
                "Volume" => self.settings.borrow().volume.to_variant(),
                "CanGoNext" | "CanGoPrevious" => (queue_available && self.queue.borrow().len() > 1).to_variant(),
                "CanPlay" => (has_track || !self.queue.borrow().is_empty() || !self.stations.borrow().is_empty()).to_variant(),
                "CanPause" => has_track.to_variant(),
                "CanSeek" => (has_track && !is_radio).to_variant(),
                "CanControl" => true.to_variant(),
                _ => "".to_variant(),
            }
        }
    }

    fn mpris_set_property(&self, name: &str, value: glib::Variant) -> bool {
        match name {
            "Volume" => {
                if let Some(value) = value.get::<f64>() {
                    self.volume_slider.set_value(value.clamp(0.0, 1.0));
                    true
                } else { false }
            },
            "Shuffle" => {
                if let Some(value) = value.get::<bool>() { self.set_shuffle(value); true } else { false }
            },
            "LoopStatus" => {
                if let Some(value) = value.get::<String>() {
                    let mode = match value.as_str() {
                        "None" => Repeat::Off,
                        "Track" => Repeat::One,
                        "Playlist" => Repeat::All,
                        _ => return false,
                    };
                    self.set_repeat(mode);
                    true
                } else { false }
            },
            _ => false,
        }
    }

    fn mpris_method(self: &Rc<Self>, interface: &str, method: &str, params: &glib::Variant) -> bool {
        // Helps distinguish a non-routed hardware key from a faulty MPRIS
        // handler. The service already works in the GNOME media popover.
        if std::env::var_os("SLATEAMP_DEBUG_MPRIS").is_some() {
            eprintln!("SlateAmp MPRIS received: {interface}.{method}");
        }
        match (interface, method) {
            (ROOT, "Raise") => self.window.present(),
            (ROOT, "Quit") => {
                self.persist_queue();
                self.audio.stop();
                if let Some(app) = self.window.application() { app.quit(); }
            },
            (PLAYER, "PlayPause") => self.toggle_play(),
            (PLAYER, "Play") => { if !self.playing.get() { self.toggle_play(); } },
            (PLAYER, "Pause") => { if self.playing.get() { self.toggle_play(); } },
            (PLAYER, "Stop") => self.stop(),
            (PLAYER, "OpenUri") => {
                if let Some(uri) = params.child_value(0).get::<String>() {
                    if crate::library::valid_stream_url(&uri) {
                        let title = uri.split('/').nth(2).unwrap_or("Internet radio").to_owned();
                        self.play_station(Track::station(title, uri));
                    } else if uri.starts_with("file://") {
                        if let Some(path) = gio::File::for_uri(&uri).path() {
                            if let Ok(track) = Track::file(&path) { self.add_tracks(vec![track]); }
                        }
                    }
                }
            },
            (PLAYER, "Next") => { if self.active_radio.borrow().is_none() { self.advance(false); } },
            (PLAYER, "Previous") => self.previous(),
            (PLAYER, "Seek") => {
                if self.active_radio.borrow().is_none() && self.current.get().is_some() {
                    let offset = params.child_value(0).get::<i64>().unwrap_or(0);
                    let pos = (self.audio.position().unwrap_or(0) as i64 * 1_000_000).saturating_add(offset);
                    self.audio.seek((pos.max(0) / 1_000_000) as u64);
                    self.mpris_seeked();
                }
            },
            (PLAYER, "SetPosition") => {
                if self.active_radio.borrow().is_none() && self.current.get().is_some() {
                    let id = params.child_value(0).get::<glib::variant::ObjectPath>();
                    let pos = params.child_value(1).get::<i64>();
                    if id.as_ref().is_some_and(|p| p.as_str() == track_path(self.mpris_serial.get())) {
                        if let Some(pos) = pos {
                            self.audio.seek((pos.max(0) / 1_000_000) as u64);
                            self.mpris_seeked();
                        }
                    }
                }
            },
            _ => return false,
        }
        true
    }

    /// Notify desktop shells when the displayed track or state changes.
    pub(crate) fn mpris_changed(&self) {
        let Some(conn) = self.mpris_connection.borrow().clone() else { return; };
        let data = glib::VariantDict::new(None);
        for name in ["PlaybackStatus", "Metadata", "CanGoNext", "CanGoPrevious", "CanPlay", "CanPause", "CanSeek", "Volume", "Shuffle", "LoopStatus"] {
            let value = self.mpris_property(PLAYER, name);
            data.insert_value(name, &value);
        }
        let args = glib::Variant::tuple_from_iter([
            PLAYER.to_variant(), data.end(), Vec::<String>::new().to_variant(),
        ]);
        if let Err(err) = conn.emit_signal(None, PATH, "org.freedesktop.DBus.Properties", "PropertiesChanged", Some(&args)) {
            eprintln!("MPRIS PropertiesChanged: {err}");
        }
    }

    fn mpris_seeked(&self) {
        let Some(conn) = self.mpris_connection.borrow().clone() else { return; };
        let pos = self.audio.position().unwrap_or(0) as i64 * 1_000_000;
        let args = (pos,).to_variant();
        let _ = conn.emit_signal(None, PATH, PLAYER, "Seeked", Some(&args));
    }

    pub(crate) fn start_mpris(self: &Rc<Self>) {
        let s = self.clone();
        gio::bus_own_name(
            gio::BusType::Session,
            BUS_NAME,
            gio::BusNameOwnerFlags::NONE,
            move |connection, _| {
                let xml = match gio::DBusNodeInfo::for_xml(include_str!("mpris.xml")) {
                    Ok(xml) => xml,
                    Err(err) => { eprintln!("MPRIS introspection XML: {err}"); return; }
                };
                for interface in [ROOT, PLAYER] {
                    let Some(info) = xml.lookup_interface(interface) else { continue; };
                    let methods = s.clone();
                    let properties = s.clone();
                    let setters = s.clone();
                    let result = connection.register_object(PATH, &info)
                        .method_call(move |_, _, _, iface, name, params, invocation| {
                            if methods.mpris_method(iface.unwrap_or(interface), name, &params) {
                                invocation.return_value(None);
                            } else {
                                invocation.return_dbus_error("org.freedesktop.DBus.Error.UnknownMethod", "Unsupported MPRIS method");
                            }
                        })
                        .property(move |_, _, _, _, name| properties.mpris_property(interface, name))
                        .set_property(move |_, _, _, _, name, value| setters.mpris_set_property(name, value))
                        .build();
                    if let Err(err) = result { eprintln!("Cannot register MPRIS {interface}: {err}"); }
                }
                s.mpris_connection.replace(Some(connection));
                s.mpris_changed();
                if std::env::var_os("SLATEAMP_DEBUG_MPRIS").is_some() {
                    eprintln!("SlateAmp MPRIS exported on {BUS_NAME}");
                }
            },
            |_, _| {
                if std::env::var_os("SLATEAMP_DEBUG_MPRIS").is_some() {
                    eprintln!("SlateAmp MPRIS bus name acquired");
                }
            },
            |_, name| eprintln!("MPRIS name unavailable: {name}"),
        );
    }
}

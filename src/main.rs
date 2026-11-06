mod audio;
mod library;
mod mpris;
mod settings;
mod spectrum;

use adw::prelude::*;
use adw::{gtk, Application, ApplicationWindow};
use gtk::{gio, glib};
use library::{MusicFolder, Track, import_playlist, load_stations, save_stations, valid_stream_url};
use settings::{EqPreset, Language, Repeat, Settings, Theme, Visualizer};
use spectrum::Spectrum;
use std::{cell::{Cell, RefCell}, rc::Rc, sync::mpsc::{self, Receiver, TryRecvError}, time::{Duration, SystemTime, UNIX_EPOCH}};

thread_local! { static APP_LANG: Cell<Language> = const { Cell::new(Language::System) }; }
fn tr(en: &'static str, ru: &'static str) -> &'static str {
    APP_LANG.with(|lang| settings::translate(lang.get(), en, ru))
}
fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    if !class.is_empty() { l.add_css_class(class); }
    l
}
fn icon_button(icon: &str, tip: &str, class: &str) -> gtk::Button {
    let btn = gtk::Button::from_icon_name(icon);
    btn.set_tooltip_text(Some(tip));
    btn.add_css_class(class);
    btn
}
fn format_time(value: u64) -> String {
    if value >= 3600 { format!("{}:{:02}:{:02}", value / 3600, (value / 60) % 60, value % 60) }
    else { format!("{}:{:02}", value / 60, value % 60) }
}
fn show_message(parent: &ApplicationWindow, title: &str, body: &str) {
    let dialog = adw::AlertDialog::builder().heading(title).body(body).build();
    dialog.add_response("ok", "OK");
    dialog.present(Some(parent));
}
fn save_settings(settings: &Settings) {
    if let Err(err) = settings.save() { eprintln!("Settings not saved: {err}"); }
}

// Reuse the same GTK widgets at every size, rather than rebuilding the lists
// (which would lose their scroll position and button connections).
#[derive(Copy, Clone, Eq, PartialEq)]
enum LayoutMode { Compact, Wide, Expanded }

struct SlateAmp {
    window: ApplicationWindow,
    audio: audio::Audio,
    settings: Rc<RefCell<Settings>>,
    queue: RefCell<Vec<Track>>,
    stations: RefCell<Vec<Track>>,
    folders: RefCell<Vec<MusicFolder>>,
    current: Cell<Option<usize>>,
    active_radio: RefCell<Option<Track>>,
    playing: Cell<bool>,
    title: gtk::Label,
    subtitle: gtk::Label,
    status: gtk::Label,
    play_button: gtk::Button,
    shuffle_button: gtk::Button,
    repeat_button: gtk::Button,
    queue_list: gtk::ListBox,
    queue_indicators: RefCell<Vec<gtk::Image>>,
    stations_list: gtk::ListBox,
    station_play_buttons: RefCell<Vec<gtk::Button>>,
    folders_list: gtk::ListBox,
    folder_status: gtk::Label,
    stack: gtk::Stack,
    preferences_window: RefCell<Option<adw::PreferencesWindow>>,
    position: gtk::Label,
    duration: gtk::Label,
    scrubber: gtk::Scale,
    scrub_blocked: Cell<bool>,
    spectrum: Spectrum,
    scan_generation: Cell<u64>,
    scan_rx: RefCell<Option<(u64, Receiver<anyhow::Result<Vec<MusicFolder>>>)>>,
    bus_watch: RefCell<Option<gst::bus::BusWatchGuard>>,
    mpris_connection: RefCell<Option<gio::DBusConnection>>,
    mpris_serial: Cell<u64>,
    volume_slider: gtk::Scale,
    layout: gtk::Box,
    player_column: gtk::Box,
    display_panel: gtk::Box,
    playlist_panel: gtk::Box,
    controls_panel: gtk::Box,
    library_side_panel: gtk::Box,
    side_folders_list: gtk::ListBox,
    layout_mode: Cell<LayoutMode>,
}

fn theme_css_class(theme: Theme) -> &'static str {
    match theme {
        Theme::Graphite => "theme-graphite",
        Theme::Classic => "theme-classic",
        Theme::Amber => "theme-amber",
        Theme::Ice => "theme-ice",
        Theme::Ocean => "theme-ocean",
        Theme::Violet => "theme-violet",
        Theme::Silver => "theme-silver",
        Theme::Phosphor => "theme-phosphor",
        Theme::AdwaitaLight => "theme-adwaita-light",
        Theme::AdwaitaDark => "theme-adwaita-dark",
        Theme::Nord => "theme-nord",
        Theme::Catppuccin => "theme-catppuccin",
        Theme::SolarizedLight => "theme-solarized-light",
    }
}

fn folder_list_row(title: &str, count: usize, all_music: bool) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    line.set_margin_start(9);
    line.set_margin_end(9);
    line.set_margin_top(5);
    line.set_margin_bottom(5);
    line.append(&gtk::Image::from_icon_name(if all_music {
        "audio-x-generic-symbolic"
    } else { "folder-symbolic" }));
    let name = label(title, "queue-title");
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_tooltip_text(Some(title));
    line.append(&name);
    line.append(&label(&count.to_string(), "track-number"));
    row.set_child(Some(&line));
    row
}

impl SlateAmp {
    fn update_responsive_layout(&self) {
        let width = self.window.width();
        if width < 1 { return; }
        // These are logical GTK pixels, not physical pixels. Wider windows
        // change composition; the spectrum and transport never become 2m wide.
        let mode = if width >= 1480 { LayoutMode::Expanded }
                   else if width >= 880 { LayoutMode::Wide }
                   else { LayoutMode::Compact };
        let previous = self.layout_mode.get();
        if mode == previous { return; }

        match previous {
            LayoutMode::Compact => {
                self.layout.remove(&self.display_panel);
                self.layout.remove(&self.playlist_panel);
                self.layout.remove(&self.controls_panel);
            }
            LayoutMode::Wide | LayoutMode::Expanded => {
                self.layout.remove(&self.player_column);
                self.layout.remove(&self.playlist_panel);
                if previous == LayoutMode::Expanded {
                    self.layout.remove(&self.library_side_panel);
                }
                self.player_column.remove(&self.display_panel);
                self.player_column.remove(&self.controls_panel);
            }
        }

        match mode {
            LayoutMode::Compact => {
                self.layout.set_orientation(gtk::Orientation::Vertical);
                self.layout.append(&self.display_panel);
                self.layout.append(&self.playlist_panel);
                self.layout.append(&self.controls_panel);
                self.player_column.set_width_request(-1);
                self.spectrum.widget.set_content_height(84);
                self.spectrum.widget.set_height_request(84);
            }
            LayoutMode::Wide | LayoutMode::Expanded => {
                self.layout.set_orientation(gtk::Orientation::Horizontal);
                self.player_column.set_width_request(390);
                self.player_column.append(&self.display_panel);
                self.player_column.append(&self.controls_panel);
                self.layout.append(&self.player_column);
                self.layout.append(&self.playlist_panel);
                if mode == LayoutMode::Expanded {
                    self.layout.append(&self.library_side_panel);
                }
                self.spectrum.widget.set_content_height(118);
                self.spectrum.widget.set_height_request(118);
            }
        }
        self.layout_mode.set(mode);
    }

    fn remember_window_geometry(&self) {
        // Actual allocated size, NOT default_width/default_height: the latter
        // do not follow manual GTK4/Wayland window resizing.
        if !self.window.is_visible() { return; }
        let width = self.window.width();
        let height = self.window.height();
        if width < 340 || height < 200 { return; } // not mapped / transitional
        let mut prefs = self.settings.borrow_mut();
        if prefs.window_width != width || prefs.window_height != height {
            prefs.window_width = width;
            prefs.window_height = height;
            save_settings(&prefs);
        }
    }

    fn persist_queue(&self) {
        if self.settings.borrow().remember_queue {
            if let Err(err) = library::save_queue(&self.queue.borrow()) {
                eprintln!("Queue could not be saved: {err}");
            }
        }
    }

    fn start_track(self: &Rc<Self>, index: usize) {
        let Some(track) = self.queue.borrow().get(index).cloned() else { return; };
        // Local music owns the queue; switching away from a radio does not insert it.
        self.active_radio.replace(None);
        self.current.set(Some(index));
        self.mpris_serial.set(self.mpris_serial.get().wrapping_add(1));
        self.title.set_text(&track.title);
        self.subtitle.set_text("");
        self.status.set_text(tr("Connecting…", "Подключение…"));
        self.scrubber.set_sensitive(!track.is_radio);
        self.scrub_blocked.set(true);
        self.scrubber.set_value(0.0);
        self.scrub_blocked.set(false);
        self.position.set_text("0:00");
        self.duration.set_text(if track.is_radio { "LIVE" } else { "--:--" });
        self.spectrum.clear();
        match self.audio.play(&track.uri) {
            Ok(()) => {
                self.playing.set(true);
                self.play_button.set_icon_name("media-playback-pause-symbolic");
                self.update_queue_playback();
                self.update_stations_playback();
                self.mpris_changed();
            }
            Err(err) => {
                self.playing.set(false);
                self.status.set_text(tr("Playback error", "Ошибка воспроизведения"));
                self.play_button.set_icon_name("media-playback-start-symbolic");
                show_message(&self.window, tr("Cannot play", "Не удалось воспроизвести"), &err.to_string());
            }
        }
    }

    fn play_station(self: &Rc<Self>, station: Track) {
        // Radio is an independent source, not a queue item.
        self.current.set(None);
        self.active_radio.replace(Some(station.clone()));
        self.mpris_serial.set(self.mpris_serial.get().wrapping_add(1));
        self.title.set_text(&station.title);
        self.subtitle.set_text("");
        self.status.set_text(tr("Connecting…", "Подключение…"));
        self.scrubber.set_sensitive(false);
        self.scrub_blocked.set(true);
        self.scrubber.set_value(0.0);
        self.scrub_blocked.set(false);
        self.position.set_text("0:00");
        self.duration.set_text("LIVE");
        self.spectrum.clear();
        match self.audio.play(&station.uri) {
            Ok(()) => {
                self.playing.set(true);
                self.play_button.set_icon_name("media-playback-pause-symbolic");
                self.update_queue_playback();
                self.update_stations_playback();
                self.mpris_changed();
            }
            Err(err) => {
                self.playing.set(false);
                self.play_button.set_icon_name("media-playback-start-symbolic");
                self.status.set_text(tr("Playback error", "Ошибка воспроизведения"));
                show_message(&self.window, tr("Cannot play", "Не удалось воспроизвести"), &err.to_string());
            }
        }
    }

    fn toggle_play(self: &Rc<Self>) {
        let has_radio = self.active_radio.borrow().is_some();
        if self.playing.get() {
            if let Err(err) = self.audio.pause() { self.status.set_text(&err.to_string()); return; }
            self.playing.set(false);
            self.status.set_text(tr("Paused", "Пауза"));
            self.play_button.set_icon_name("media-playback-start-symbolic");
            self.spectrum.clear();
            self.update_stations_playback();
            self.mpris_changed();
        } else if self.current.get().is_some() || has_radio {
            if let Err(err) = self.audio.resume() { self.status.set_text(&err.to_string()); return; }
            self.playing.set(true);
            self.status.set_text("");
            self.play_button.set_icon_name("media-playback-pause-symbolic");
            self.update_stations_playback();
            self.mpris_changed();
        } else if self.stack.visible_child_name().as_deref() == Some("radio") {
            let station = self.stations.borrow().first().cloned();
            if let Some(station) = station { self.play_station(station); }
        } else if !self.queue.borrow().is_empty() {
            self.start_track(0);
        }
    }

    fn stop(self: &Rc<Self>) {
        self.audio.stop();
        self.playing.set(false);
        self.current.set(None);
        self.active_radio.replace(None);
        self.play_button.set_icon_name("media-playback-start-symbolic");
        self.status.set_text("");
        self.spectrum.clear();
        self.position.set_text("0:00");
        self.scrubber.set_sensitive(false);
        self.update_queue_playback();
        self.update_stations_playback();
        self.mpris_changed();
    }

    fn advance(self: &Rc<Self>, eos: bool) {
        // A radio stream must never auto-advance into the local music queue.
        let radio_was_active = self.active_radio.borrow().is_some();
        if radio_was_active { self.stop(); return; }
        let len = self.queue.borrow().len();
        if len == 0 { self.stop(); return; }
        let prefs = self.settings.borrow();
        if eos && prefs.repeat == Repeat::One {
            let index = self.current.get().unwrap_or(0);
            drop(prefs);
            self.start_track(index);
            return;
        }
        let next = if prefs.shuffle && len > 1 {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos() as usize;
            let offset = 1 + (nanos % (len - 1));
            Some((self.current.get().unwrap_or(0) + offset) % len)
        } else {
            let target = self.current.get().map(|i| i + 1).unwrap_or(0);
            if target < len { Some(target) }
            else if !eos || prefs.repeat == Repeat::All { Some(0) }
            else { None }
        };
        drop(prefs);
        if let Some(index) = next { self.start_track(index); }
        else { self.stop(); }
    }

    fn previous(self: &Rc<Self>) {
        if self.active_radio.borrow().is_some() { return; }
        let len = self.queue.borrow().len();
        if len == 0 { return; }
        let prev = self.current.get().map(|i| if i == 0 { len - 1 } else { i - 1 }).unwrap_or(0);
        self.start_track(prev);
    }

    fn add_tracks(self: &Rc<Self>, tracks: Vec<Track>) {
        if tracks.is_empty() { return; }
        let (radio, music): (Vec<_>, Vec<_>) = tracks.into_iter().partition(|t| t.is_radio);
        if !radio.is_empty() {
            let mut stations = self.stations.borrow_mut();
            let mut changed = false;
            for station in &radio {
                if !stations.iter().any(|x| x.uri == station.uri) {
                    stations.push(station.clone());
                    changed = true;
                }
            }
            if changed {
                if let Err(err) = save_stations(&stations) { eprintln!("Stations not saved: {err}"); }
            }
            drop(stations);
            self.refresh_stations();
        }
        if !music.is_empty() {
            let start = self.queue.borrow().len();
            self.queue.borrow_mut().extend(music);
            self.persist_queue();
            self.refresh_queue();
            self.stack.set_visible_child_name("queue");
            if self.settings.borrow().autoplay_import { self.start_track(start); }
        } else if let Some(station) = radio.first() {
            self.stack.set_visible_child_name("radio");
            if self.settings.borrow().autoplay_import { self.play_station(station.clone()); }
        }
    }

    fn remove_from_queue(self: &Rc<Self>, index: usize) {
        if index >= self.queue.borrow().len() { return; }
        self.queue.borrow_mut().remove(index);
        match self.current.get() {
            Some(current) if current == index => self.stop(),
            Some(current) if current > index => self.current.set(Some(current - 1)),
            _ => {}
        }
        self.persist_queue();
        self.refresh_queue();
    }

    fn play_folder(self: &Rc<Self>, index: usize) {
        let paths: Vec<_> = if index == 0 {
            self.folders.borrow().iter().flat_map(|folder| folder.files.clone()).collect()
        } else {
            self.folders.borrow().get(index - 1).map(|f| f.files.clone()).unwrap_or_default()
        };
        let tracks: Vec<_> = paths.iter().filter_map(|p| Track::file(p).ok()).collect();
        if tracks.is_empty() { return; }
        self.stop();
        *self.queue.borrow_mut() = tracks;
        self.persist_queue();
        self.refresh_queue();
        self.stack.set_visible_child_name("queue");
        self.start_track(0);
    }

    // Playback changes must not destroy and recreate the ListBox rows: GTK would
    // reset the ScrolledWindow adjustment, jumping back to the first track.
    fn update_queue_playback(&self) {
        let selected = if self.active_radio.borrow().is_none() { self.current.get() } else { None };
        for (i, icon) in self.queue_indicators.borrow().iter().enumerate() {
            let active = selected == Some(i);
            icon.set_visible(active);
            if let Some(row) = self.queue_list.row_at_index(i as i32) {
                if active { row.add_css_class("playing-row"); }
                else { row.remove_css_class("playing-row"); }
            }
        }
    }

    fn refresh_queue(self: &Rc<Self>) {
        // Called only when tracks are added, removed, or an album replaces the queue.
        while let Some(row) = self.queue_list.first_child() { self.queue_list.remove(&row); }
        let mut indicators = Vec::new();
        for (i, track) in self.queue.borrow().iter().enumerate() {
            let row = gtk::ListBoxRow::new();
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            line.set_margin_start(8);
            line.set_margin_end(8);
            line.set_margin_top(5);
            line.set_margin_bottom(5);
            let name = label(&track.title, "queue-title");
            name.set_hexpand(true);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            line.append(&name);
            let indicator = gtk::Image::from_icon_name("audio-volume-high-symbolic");
            indicator.add_css_class("playing-indicator");
            indicator.set_visible(false);
            line.append(&indicator);
            indicators.push(indicator);
            let remove = icon_button("user-trash-symbolic", tr("Remove track", "Удалить трек"), "flat");
            remove.add_css_class("queue-remove");
            remove.set_focus_on_click(false);
            let s = self.clone();
            remove.connect_clicked(move |_| s.remove_from_queue(i));
            line.append(&remove);
            row.set_child(Some(&line));
            self.queue_list.append(&row);
        }
        self.queue_indicators.replace(indicators);
        self.update_queue_playback();
        self.mpris_changed();
    }

    // Same rule for internet radio: switching stations must not reset its scrollbar.
    fn update_stations_playback(&self) {
        for (i, station) in self.stations.borrow().iter().enumerate() {
            let active = self.active_radio.borrow().as_ref().is_some_and(|r| r.uri == station.uri);
            if let Some(btn) = self.station_play_buttons.borrow().get(i) {
                btn.set_icon_name(if active && self.playing.get() {
                    "media-playback-pause-symbolic"
                } else { "media-playback-start-symbolic" });
            }
            if let Some(row) = self.stations_list.row_at_index(i as i32) {
                if active { row.add_css_class("playing-row"); }
                else { row.remove_css_class("playing-row"); }
            }
        }
    }

    fn refresh_stations(self: &Rc<Self>) {
        while let Some(row) = self.stations_list.first_child() { self.stations_list.remove(&row); }
        let mut play_buttons = Vec::new();
        for station in self.stations.borrow().clone() {
            let row = gtk::ListBoxRow::new();
            row.set_activatable(false);
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 5);
            line.set_margin_start(6);
            line.set_margin_end(6);
            line.set_margin_top(3);
            line.set_margin_bottom(3);
            let play = icon_button("media-playback-start-symbolic", tr("Play/Pause station", "Играть/Пауза"), "flat");
            play_buttons.push(play.clone());
            let name = label(&station.title, "queue-title");
            name.set_hexpand(true);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let remove = icon_button("user-trash-symbolic", tr("Remove station", "Удалить станцию"), "flat");
            line.append(&play);
            line.append(&name);
            line.append(&remove);
            row.set_child(Some(&line));
            self.stations_list.append(&row);
            let s = self.clone();
            let station_play = station.clone();
            play.connect_clicked(move |_| {
                let active = s.active_radio.borrow().as_ref().is_some_and(|r| r.uri == station_play.uri);
                if active { s.toggle_play(); } else { s.play_station(station_play.clone()); }
            });
            let s = self.clone();
            remove.connect_clicked(move |_| {
                s.stations.borrow_mut().retain(|t| t.uri != station.uri);
                let active = s.active_radio.borrow().as_ref().is_some_and(|r| r.uri == station.uri);
                if active { s.stop(); }
                if let Err(err) = save_stations(&s.stations.borrow()) {
                    show_message(&s.window, tr("Save failed", "Ошибка сохранения"), &err.to_string());
                }
                s.refresh_stations();
            });
        }
        self.station_play_buttons.replace(play_buttons);
        self.update_stations_playback();
        self.mpris_changed();
    }

    fn refresh_folders(self: &Rc<Self>) {
        // The fullscreen library sidebar is a second view of the same data.
        // Both views share the same play_folder() handler.
        for view in [&self.folders_list, &self.side_folders_list] {
            while let Some(row) = view.first_child() { view.remove(&row); }
            let folders = self.folders.borrow();
            if folders.is_empty() { continue; }
            let total = folders.iter().map(|f| f.files.len()).sum::<usize>();
            view.append(&folder_list_row(tr("All music", "Вся музыка"), total, true));
            for folder in folders.iter() {
                let name = if folder.relative_name == "." {
                    tr("Root folder", "Корневая папка")
                } else { &folder.relative_name };
                view.append(&folder_list_row(name, folder.files.len(), false));
            }
        }
    }

    fn scan_library(self: &Rc<Self>) {
        let root = self.settings.borrow().music_root.clone();
        self.folders.borrow_mut().clear();
        self.refresh_folders();
        self.scan_generation.set(self.scan_generation.get() + 1);
        let id = self.scan_generation.get();
        if let Some(root) = root {
            self.folder_status.set_text(tr("Scanning…", "Сканирование…"));
            let (sender, rx) = mpsc::channel();
            self.scan_rx.replace(Some((id, rx)));
            std::thread::spawn(move || {
                let result = library::scan_music(&root);
                let _ = sender.send(result);
            });
        } else {
            self.scan_rx.replace(None);
            self.folder_status.set_text(tr("Choose a music folder in Settings", "Выберите папку в настройках"));
        }
    }

    fn poll_scan(self: &Rc<Self>) {
        let outcome = {
            let receiver = self.scan_rx.borrow();
            if let Some((id, rx)) = receiver.as_ref() {
                match rx.try_recv() {
                    Ok(result) => Some((*id, Some(result))),
                    Err(TryRecvError::Disconnected) => Some((*id, None)),
                    Err(TryRecvError::Empty) => None,
                }
            } else { None }
        };
        if let Some((id, outcome)) = outcome {
            self.scan_rx.replace(None);
            if id != self.scan_generation.get() { return; }
            match outcome {
                Some(Ok(folders)) => {
                    let count = folders.iter().map(|f| f.files.len()).sum::<usize>();
                    self.folder_status.set_text(&format!("{} · {}", folders.len(), count));
                    *self.folders.borrow_mut() = folders;
                    self.refresh_folders();
                }
                Some(Err(err)) => self.folder_status.set_text(&err.to_string()),
                None => self.folder_status.set_text(tr("Scan failed", "Ошибка сканирования")),
            }
        }
    }

    fn open_files(self: &Rc<Self>) {
        let chooser = gtk::FileChooserNative::new(
            Some(tr("Add music or playlist", "Добавить музыку или плейлист")), Some(&self.window),
            gtk::FileChooserAction::Open, Some(tr("Open", "Открыть")), Some(tr("Cancel", "Отмена")),
        );
        chooser.set_select_multiple(true);
        let s = self.clone();
        chooser.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                let files = dialog.files();
                let mut tracks = Vec::new();
                let mut errors = Vec::new();
                for i in 0..files.n_items() {
                    let Some(file) = files.item(i).and_then(|v| v.downcast::<gio::File>().ok()) else { continue; };
                    let Some(path) = file.path() else { continue; };
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
                    if ["m3u", "m3u8", "pls"].contains(&ext.as_str()) {
                        match import_playlist(&path) { Ok(imported) => tracks.extend(imported), Err(e) => errors.push(e.to_string()) }
                    } else {
                        match Track::file(&path) { Ok(track) => tracks.push(track), Err(e) => errors.push(e.to_string()) }
                    }
                }
                if !tracks.is_empty() { s.add_tracks(tracks); }
                if !errors.is_empty() {
                    show_message(&s.window, tr("Some files could not be imported", "Некоторые файлы не добавлены"), &errors.join("\n"));
                }
            }
            dialog.destroy();
        });
        chooser.show();
    }

    fn add_station_dialog(self: &Rc<Self>) {
        let dialog = gtk::Dialog::builder()
            .title(tr("Add internet radio", "Добавить радиостанцию"))
            .transient_for(&self.window).modal(true).default_width(410).build();
        dialog.add_button(tr("Cancel", "Отмена"), gtk::ResponseType::Cancel);
        dialog.add_button(tr("Add", "Добавить"), gtk::ResponseType::Accept);
        let body = dialog.content_area();
        body.set_spacing(8);
        body.set_margin_top(12);
        body.set_margin_bottom(12);
        body.set_margin_start(14);
        body.set_margin_end(14);
        let name = gtk::Entry::new();
        name.set_placeholder_text(Some(tr("Station name", "Название станции")));
        let url = gtk::Entry::new();
        url.set_placeholder_text(Some("https://…"));
        body.append(&name);
        body.append(&url);
        dialog.set_default_response(gtk::ResponseType::Accept);
        let s = self.clone();
        dialog.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                let title = name.text().trim().to_string();
                let uri = url.text().trim().to_string();
                if title.is_empty() || !valid_stream_url(&uri) {
                    show_message(&s.window, tr("Invalid station", "Некорректная станция"),
                        tr("Enter a name and HTTP(S) stream URL.", "Укажите название и HTTP(S)-адрес потока."));
                } else { s.add_tracks(vec![Track::station(title, uri)]); }
            }
            dialog.close();
        });
        dialog.present();
    }

    fn select_root(self: &Rc<Self>, display: adw::ActionRow, parent: &adw::PreferencesWindow) {
        let chooser = gtk::FileChooserNative::new(
            Some(tr("Music folder", "Папка с музыкой")), Some(parent),
            gtk::FileChooserAction::SelectFolder,
            Some(tr("Select", "Выбрать")), Some(tr("Cancel", "Отмена")),
        );
        let s = self.clone();
        chooser.connect_response(move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(path) = dialog.file().and_then(|f| f.path()) {
                    s.settings.borrow_mut().music_root = Some(path.clone());
                    save_settings(&s.settings.borrow());
                    display.set_subtitle(&path.to_string_lossy());
                    s.scan_library();
                }
            }
            dialog.destroy();
        });
        chooser.show();
    }

    fn set_theme(&self, theme: Theme) {
        for candidate in [Theme::Graphite, Theme::Classic, Theme::Amber, Theme::Ice,
                          Theme::Ocean, Theme::Violet, Theme::Silver, Theme::Phosphor,
                          Theme::AdwaitaLight, Theme::AdwaitaDark, Theme::Nord,
                          Theme::Catppuccin, Theme::SolarizedLight] {
            self.window.remove_css_class(theme_css_class(candidate));
        }
        self.window.add_css_class(theme_css_class(theme));
        // Player skins are local to the SlateAmp window. Never change the
        // process-wide Adwaita color scheme: the separate preferences window
        // must respect the user's GNOME light/dark appearance setting.
        self.spectrum.set_theme(theme);
    }

    fn set_shuffle(&self, enabled: bool) {
        self.settings.borrow_mut().shuffle = enabled;
        self.shuffle_button.set_css_classes(if enabled { &["transport-secondary", "on"] } else { &["transport-secondary"] });
        save_settings(&self.settings.borrow());
        self.mpris_changed();
    }

    fn set_repeat(&self, repeat: Repeat) {
        self.settings.borrow_mut().repeat = repeat;
        self.repeat_button.set_css_classes(if repeat == Repeat::Off { &["transport-secondary"] } else { &["transport-secondary", "on"] });
        let tooltip = match repeat {
            Repeat::Off => tr("Repeat off", "Повтор выключен"),
            Repeat::All => tr("Repeat all", "Повтор очереди"),
            Repeat::One => tr("Repeat one", "Повтор трека"),
        };
        self.repeat_button.set_tooltip_text(Some(tooltip));
        save_settings(&self.settings.borrow());
        self.mpris_changed();
    }

    fn preferences(self: &Rc<Self>) {
        // A standalone top-level window: not modal and not transient to the player.
        // Reuse it rather than opening a new copy on every click.
        if let Some(window) = self.preferences_window.borrow().as_ref() {
            window.present();
            return;
        }

        let window = adw::PreferencesWindow::new();
        window.set_title(Some(tr("Settings", "Настройки")));
        window.set_default_size(540, 530);
        window.set_search_enabled(false);
        window.set_modal(false);
        if let Some(app) = self.window.application() {
            window.set_application(Some(&app));
        }

        let appearance = adw::PreferencesPage::new();
        appearance.set_title(tr("Appearance", "Внешний вид"));
        appearance.set_icon_name(Some("preferences-desktop-theme-symbolic"));
        let look = adw::PreferencesGroup::new();
        look.set_title(tr("Interface", "Интерфейс"));

        let lang = combo_setting(tr("Language", "Язык"), &[
            tr("System", "Системный"), "Русский", "English",
        ], match self.settings.borrow().language {
            Language::System => 0, Language::Russian => 1, Language::English => 2,
        });
        lang.set_subtitle(tr("Requires a restart", "Применяется после перезапуска"));
        look.add(&lang);
        let settings = self.settings.clone();
        lang.connect_selected_notify(move |combo| {
            settings.borrow_mut().language = match combo.selected() {
                1 => Language::Russian, 2 => Language::English, _ => Language::System,
            };
            save_settings(&settings.borrow());
        });

        let theme = combo_setting(tr("Theme", "Тема"), &[
            tr("Graphite", "Графит"), "Winamp Green",
            tr("Amber", "Янтарь"), tr("Arctic Light", "Арктика"),
            tr("Deep Ocean", "Океан"), tr("Ultraviolet", "Ультрафиолет"),
            tr("Retro Silver", "Ретро-серебро"), tr("Phosphor LCD", "Люминофор"),
            "Adwaita Light", "Adwaita Dark", "Nord", "Catppuccin Mocha", "Solarized Light",
        ], match self.settings.borrow().theme {
            Theme::Graphite => 0, Theme::Classic => 1, Theme::Amber => 2,
            Theme::Ice => 3, Theme::Ocean => 4, Theme::Violet => 5,
            Theme::Silver => 6, Theme::Phosphor => 7,
            Theme::AdwaitaLight => 8, Theme::AdwaitaDark => 9,
            Theme::Nord => 10, Theme::Catppuccin => 11, Theme::SolarizedLight => 12,
        });
        look.add(&theme);
        let weak = Rc::downgrade(self);
        theme.connect_selected_notify(move |combo| {
            if let Some(s) = weak.upgrade() {
                let mode = match combo.selected() {
                    1 => Theme::Classic, 2 => Theme::Amber, 3 => Theme::Ice,
                    4 => Theme::Ocean, 5 => Theme::Violet, 6 => Theme::Silver,
                    7 => Theme::Phosphor, 8 => Theme::AdwaitaLight,
                    9 => Theme::AdwaitaDark, 10 => Theme::Nord,
                    11 => Theme::Catppuccin, 12 => Theme::SolarizedLight,
                    _ => Theme::Graphite,
                };
                s.settings.borrow_mut().theme = mode;
                s.set_theme(mode);
                save_settings(&s.settings.borrow());
            }
        });

        let visual = combo_setting(tr("Visualization", "Визуализация"), &[
            tr("Segments", "Сегменты"), tr("Bars", "Столбцы"),
            tr("Frequency curve", "Частотная линия"), tr("Off", "Выключено"),
            tr("Mirror", "Зеркало"), tr("Radial spectrum", "Круговой спектр"),
        ], match self.settings.borrow().visualizer {
            Visualizer::Segments => 0, Visualizer::Bars => 1,
            Visualizer::Curve => 2, Visualizer::Off => 3,
            Visualizer::Mirror => 4, Visualizer::Radial => 5,
        });
        look.add(&visual);
        let weak = Rc::downgrade(self);
        visual.connect_selected_notify(move |combo| {
            if let Some(s) = weak.upgrade() {
                let mode = match combo.selected() {
                    1 => Visualizer::Bars, 2 => Visualizer::Curve,
                    3 => Visualizer::Off, 4 => Visualizer::Mirror,
                    5 => Visualizer::Radial, _ => Visualizer::Segments,
                };
                s.settings.borrow_mut().visualizer = mode;
                s.spectrum.set_mode(mode);
                save_settings(&s.settings.borrow());
            }
        });
        appearance.add(&look);
        window.add(&appearance);

        let library = adw::PreferencesPage::new();
        library.set_title(tr("Library & playback", "Музыка и поведение"));
        library.set_icon_name(Some("folder-music-symbolic"));
        let files = adw::PreferencesGroup::new();
        files.set_title(tr("Music library", "Медиатека"));

        let music_root = adw::ActionRow::new();
        music_root.set_title(tr("Music folder", "Папка с музыкой"));
        let path = self.settings.borrow().music_root.as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| tr("Not selected", "Не выбрана").to_string());
        music_root.set_subtitle(&path);
        let choose = gtk::Button::with_label(tr("Choose…", "Выбрать…"));
        choose.set_valign(gtk::Align::Center);
        music_root.add_suffix(&choose);
        music_root.set_activatable_widget(Some(&choose));
        files.add(&music_root);
        let weak = Rc::downgrade(self);
        let weak_window = window.downgrade();
        choose.connect_clicked(move |_| {
            if let (Some(s), Some(parent)) = (weak.upgrade(), weak_window.upgrade()) {
                s.select_root(music_root.clone(), &parent);
            }
        });

        let rescan = adw::ActionRow::new();
        rescan.set_title(tr("Rescan music", "Пересканировать музыку"));
        let rescan_button = gtk::Button::from_icon_name("view-refresh-symbolic");
        rescan_button.set_tooltip_text(Some(tr("Rescan library", "Пересканировать медиатеку")));
        rescan_button.add_css_class("flat");
        rescan_button.set_valign(gtk::Align::Center);
        rescan.add_suffix(&rescan_button);
        rescan.set_activatable_widget(Some(&rescan_button));
        files.add(&rescan);
        let weak = Rc::downgrade(self);
        rescan_button.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() { s.scan_library(); }
        });
        library.add(&files);

        let behavior = adw::PreferencesGroup::new();
        behavior.set_title(tr("Playback", "Воспроизведение"));
        let autoplay = adw::SwitchRow::new();
        autoplay.set_title(tr("Play newly added tracks", "Играть добавленные треки"));
        autoplay.set_active(self.settings.borrow().autoplay_import);
        behavior.add(&autoplay);
        let settings = self.settings.clone();
        autoplay.connect_active_notify(move |row| {
            settings.borrow_mut().autoplay_import = row.is_active();
            save_settings(&settings.borrow());
        });

        let remember = adw::SwitchRow::new();
        remember.set_title(tr("Remember queue", "Запоминать очередь"));
        remember.set_active(self.settings.borrow().remember_queue);
        behavior.add(&remember);
        let weak = Rc::downgrade(self);
        remember.connect_active_notify(move |row| {
            if let Some(s) = weak.upgrade() {
                s.settings.borrow_mut().remember_queue = row.is_active();
                save_settings(&s.settings.borrow());
                if row.is_active() { s.persist_queue(); }
                else { let _ = std::fs::remove_file(library::queue_file()); }
            }
        });

        let background = adw::SwitchRow::new();
        background.set_title(tr("Keep playing after closing window", "Играть после закрытия окна"));
        background.set_active(self.settings.borrow().background_on_close);
        behavior.add(&background);
        let settings = self.settings.clone();
        background.connect_active_notify(move |row| {
            settings.borrow_mut().background_on_close = row.is_active();
            save_settings(&settings.borrow());
        });
        library.add(&behavior);
        window.add(&library);

        let weak = Rc::downgrade(self);
        window.connect_close_request(move |_| {
            if let Some(s) = weak.upgrade() { s.preferences_window.borrow_mut().take(); }
            glib::Propagation::Proceed
        });
        self.preferences_window.replace(Some(window.clone()));
        window.present();
    }

    fn handle_message(self: &Rc<Self>, message: &gst::Message) {
        match message.view() {
            gst::MessageView::Eos(..) => self.advance(true),
            gst::MessageView::Error(err) => {
                self.playing.set(false);
                self.play_button.set_icon_name("media-playback-start-symbolic");
                self.status.set_text(tr("Stream error", "Ошибка потока"));
                self.spectrum.clear();
                eprintln!("GStreamer: {} ({:?})", err.error(), err.debug());
                self.subtitle.set_text(&err.error().to_string());
                self.mpris_changed();
            }
            gst::MessageView::Tag(tag) => {
                let tags = tag.tags();
                let title = tags.get::<gst::tags::Title>().map(|t| t.get().to_string());
                let artist = tags.get::<gst::tags::Artist>().map(|t| t.get().to_string());
                if let Some(title) = title {
                    self.subtitle.set_text(&artist.map(|name| format!("{name} — {title}")).unwrap_or(title));
                self.mpris_changed();
                }
            }
            gst::MessageView::Element(element) => {
                if let Some(structure) = element.structure() {
                    if structure.name() == "spectrum" {
                        if let Ok(list) = structure.get::<gst::List>("magnitude") {
                            let db: Vec<f64> = list.as_slice().iter()
                                .map(|v| v.get::<f32>().map(|n| n as f64).unwrap_or(-90.0))
                                .collect();
                            if self.playing.get() { self.spectrum.update(&db); }
                        }
                    }
                }
            }
            gst::MessageView::StateChanged(change) => {
                if let Some(source) = message.src() {
                    if source.type_().name().contains("GstPlayBin") && change.current() == gst::State::Playing {
                        self.status.set_text("");
                    }
                }
            }
            _ => {}
        }
    }

    fn tick(&self) {
        if !self.playing.get() { return; }
        if let Some(pos) = self.audio.position() { self.position.set_text(&format_time(pos)); }
        let is_radio = self.active_radio.borrow().is_some();
        if is_radio { self.duration.set_text("LIVE"); return; }
        if let Some(dur) = self.audio.duration().filter(|d| *d > 0) {
            self.duration.set_text(&format_time(dur));
            self.scrub_blocked.set(true);
            self.scrubber.set_range(0.0, dur as f64);
            if let Some(pos) = self.audio.position() { self.scrubber.set_value(pos as f64); }
            self.scrub_blocked.set(false);
        }
    }
}

fn combo_setting(title: &str, items: &[&str], selected: u32) -> adw::ComboRow {
    let row = adw::ComboRow::new();
    row.set_title(title);
    row.set_model(Some(&gtk::StringList::new(items)));
    row.set_selected(selected);
    row
}

fn create_equalizer(audio: &audio::Audio, settings: Rc<RefCell<Settings>>) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_label("EQ");
    button.add_css_class("eq-button");
    button.set_tooltip_text(Some(tr("Equalizer", "Эквалайзер")));
    let popover = gtk::Popover::new();
    popover.add_css_class("eq-popover");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 9);
    root.set_margin_start(14);
    root.set_margin_end(14);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let heading = label(tr("Equalizer", "Эквалайзер"), "eq-heading");
    heading.set_hexpand(true);
    head.append(&heading);
    let preset = gtk::DropDown::from_strings(&[
        tr("Custom", "Свой"), tr("Flat", "Нейтральный"), "Rock",
        tr("Bass boost", "Бас"), tr("Vocals", "Вокал"),
        "Pop", tr("Electronic", "Электроника"), "Jazz",
        tr("Classical", "Классика"), tr("Acoustic", "Акустика"),
        tr("Treble boost", "Высокие частоты"),
    ]);
    let selected_preset = settings.borrow().eq_preset;
    preset.set_selected(selected_preset.index());
    head.append(&preset);
    root.append(&head);
    let scale_line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let freqs = ["29", "59", "119", "237", "474", "947", "1.9k", "3.8k", "7.5k", "15k"];
    let sliders: Rc<RefCell<Vec<gtk::Scale>>> = Rc::new(RefCell::new(Vec::new()));
    let applying_preset = Rc::new(Cell::new(false));
    let eq_element = audio.equalizer_element();
    let initial = settings.borrow().eq_gains;
    for (i, freq) in freqs.iter().enumerate() {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
        col.set_halign(gtk::Align::Center);
        let gain = gtk::Scale::with_range(gtk::Orientation::Vertical, -12.0, 12.0, 1.0);
        gain.set_draw_value(false);
        gain.set_size_request(30, 138);
        gain.set_vexpand(false);
        gain.set_value(initial[i]);
        eq_element.set_property(&format!("band{i}"), initial[i]);
        let eq = eq_element.clone();
        let settings_for_slider = settings.clone();
        let preset_picker = preset.clone();
        let preset_busy = applying_preset.clone();
        gain.connect_value_changed(move |scale| {
            let value = scale.value();
            eq.set_property(&format!("band{i}"), value);
            if preset_busy.get() { return; }
            {
                let mut conf = settings_for_slider.borrow_mut();
                conf.eq_gains[i] = value;
                conf.eq_preset = EqPreset::Custom;
            }
            preset_picker.set_selected(EqPreset::Custom.index());
            save_settings(&settings_for_slider.borrow());
        });
        sliders.borrow_mut().push(gain.clone());
        let freq_label = label(freq, "eq-label");
        freq_label.set_halign(gtk::Align::Center);
        col.append(&gain);
        col.append(&freq_label);
        scale_line.append(&col);
    }
    root.append(&scale_line);
    let sliders_for_preset = sliders.clone();
    let preset_busy = applying_preset.clone();
    preset.connect_selected_notify(move |combo| {
        let selected = EqPreset::from_index(combo.selected());
        let Some(values) = selected.gains() else { return; };
        preset_busy.set(true);
        for (scale, value) in sliders_for_preset.borrow().iter().zip(values) {
            scale.set_value(value);
        }
        preset_busy.set(false);
        {
            let mut conf = settings.borrow_mut();
            conf.eq_gains = values;
            conf.eq_preset = selected;
        }
        save_settings(&settings.borrow());
    });
    popover.set_child(Some(&root));
    button.set_popover(Some(&popover));
    button
}

fn build_ui(app: &Application) -> anyhow::Result<Rc<SlateAmp>> {
    let audio = audio::Audio::new()?;
    let settings = Rc::new(RefCell::new(Settings::load()));
    APP_LANG.with(|lang| lang.set(settings.borrow().language));
    let initial = settings.borrow().clone();
    audio.set_volume(initial.volume);
    let stations = load_stations();
    let stations = if stations.is_empty() && !library::data_file().exists() {
        vec![Track::station("Alternative Rock", "https://alternative-rock.stream.laut.fm/alternative-rock")]
    } else { stations };
    // Migrate playlists saved by 0.3: remove radio entries from the music queue,
    // and preserve any stations that the old version accidentally saved there.
    let mut stations = stations;
    let saved = if initial.remember_queue { library::load_queue() } else { Vec::new() };
    let mut queue = Vec::new();
    let mut migrated = false;
    for track in saved {
        if track.is_radio {
            migrated = true;
            if !stations.iter().any(|s| s.uri == track.uri) { stations.push(track); }
        } else { queue.push(track); }
    }
    if migrated {
        if let Err(err) = library::save_queue(&queue) { eprintln!("Queue migration failed: {err}"); }
        if let Err(err) = save_stations(&stations) { eprintln!("Station migration failed: {err}"); }
    }

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outer.add_css_class("slate-root");
    let header = adw::HeaderBar::new();
    header.add_css_class("slate-header");
    let name = label("SlateAmp", "app-name");
    name.set_halign(gtk::Align::Center);
    header.set_title_widget(Some(&name));
    let open = icon_button("document-open-symbolic", tr("Open music or playlist", "Открыть музыку или плейлист"), "flat");
    let add_station = icon_button("list-add-symbolic", tr("Add radio station", "Добавить радиостанцию"), "flat");
    let preferences = icon_button("emblem-system-symbolic", tr("Preferences", "Настройки"), "flat");
    let quit = icon_button("application-exit-symbolic", tr("Quit", "Выход"), "flat");
    header.pack_start(&open);
    header.pack_start(&add_station);
    header.pack_end(&quit);
    header.pack_end(&preferences);
    outer.append(&header);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 7);
    body.set_margin_start(10);
    body.set_margin_end(10);
    body.set_margin_top(8);
    body.set_margin_bottom(9);
    body.set_hexpand(true);
    body.set_vexpand(true);
    // Prevent ultrawide monitors from stretching 15 track titles across 3K.
    // The background still fills the window; only content is clamped.
    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(1600);
    clamp.set_tightening_threshold(1480);
    clamp.set_hexpand(true);
    clamp.set_vexpand(true);
    clamp.set_child(Some(&body));
    outer.append(&clamp);

    let layout = gtk::Box::new(gtk::Orientation::Vertical, 8);
    layout.set_hexpand(true);
    layout.set_vexpand(true);
    body.append(&layout);
    let player_column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    player_column.set_hexpand(false);
    player_column.set_vexpand(false);

    let display = gtk::Box::new(gtk::Orientation::Vertical, 7);
    display.add_css_class("display-panel");
    display.set_vexpand(false); // v0.2 stretched this entire panel to fill the window.
    let info = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let details = gtk::Box::new(gtk::Orientation::Vertical, 1);
    details.set_hexpand(true);
    let title = label(tr("Nothing playing", "Ничего не играет"), "track-heading");
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let subtitle = label("", "track-detail");
    subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
    details.append(&title);
    details.append(&subtitle);
    let status = label("", "status-label");
    status.set_valign(gtk::Align::Start);
    info.append(&details);
    info.append(&status);
    display.append(&info);
    let spectrum = Spectrum::new(initial.visualizer, initial.theme);
    display.append(&spectrum.widget);
    layout.append(&display);

    let panel = gtk::Box::new(gtk::Orientation::Vertical, 4);
    panel.add_css_class("playlist-panel");
    panel.set_vexpand(true);
    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.set_hexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    let queue_list = gtk::ListBox::new();
    queue_list.set_selection_mode(gtk::SelectionMode::None);
    queue_list.add_css_class("track-list");
    let queue_scroll = gtk::ScrolledWindow::new();
    queue_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    queue_scroll.set_min_content_height(80);
    queue_scroll.set_child(Some(&queue_list));
    stack.add_titled(&queue_scroll, Some("queue"), tr("Queue", "Очередь"));
    let station_list = gtk::ListBox::new();
    station_list.set_selection_mode(gtk::SelectionMode::None);
    station_list.add_css_class("track-list");
    let stations_scroll = gtk::ScrolledWindow::new();
    stations_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    stations_scroll.set_child(Some(&station_list));
    let folders_list = gtk::ListBox::new();
    folders_list.set_selection_mode(gtk::SelectionMode::None);
    folders_list.add_css_class("track-list");
    let folders_scroll = gtk::ScrolledWindow::new();
    folders_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    folders_scroll.set_vexpand(true);
    folders_scroll.set_child(Some(&folders_list));
    let music_tab = gtk::Box::new(gtk::Orientation::Vertical, 3);
    music_tab.set_vexpand(true);
    music_tab.append(&folders_scroll);
    let folder_status = label("", "folder-status");
    music_tab.append(&folder_status);
    stack.add_titled(&music_tab, Some("music"), tr("Music", "Музыка"));
    stack.add_titled(&stations_scroll, Some("radio"), tr("Radio", "Радио"));
    let tabs = gtk::StackSwitcher::new();
    tabs.set_stack(Some(&stack));
    tabs.set_halign(gtk::Align::Fill);
    panel.append(&tabs);
    panel.append(&stack);
    layout.append(&panel);

    let controls = gtk::Box::new(gtk::Orientation::Vertical, 4);
    controls.add_css_class("controls-panel");
    controls.set_vexpand(false);
    let scrub_line = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    let position = label("0:00", "time-label");
    let scrubber = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    scrubber.set_draw_value(false);
    scrubber.set_hexpand(true);
    scrubber.set_sensitive(false);
    let duration = label("--:--", "time-label");
    scrub_line.append(&position);
    scrub_line.append(&scrubber);
    scrub_line.append(&duration);
    controls.append(&scrub_line);

    let transport = gtk::Box::new(gtk::Orientation::Horizontal, 3);
    let prev = icon_button("media-skip-backward-symbolic", tr("Previous", "Предыдущий"), "transport-secondary");
    let play = icon_button("media-playback-start-symbolic", tr("Play/Pause · Space", "Играть/Пауза · Пробел"), "play-button");
    let stop = icon_button("media-playback-stop-symbolic", tr("Stop", "Стоп"), "transport-secondary");
    let next = icon_button("media-skip-forward-symbolic", tr("Next", "Следующий"), "transport-secondary");
    transport.append(&prev);
    transport.append(&play);
    transport.append(&stop);
    transport.append(&next);
    let shuffle = icon_button("media-playlist-shuffle-symbolic", tr("Shuffle", "Перемешать"), "transport-secondary");
    let repeat = icon_button("media-playlist-repeat-symbolic", tr("Repeat", "Повтор"), "transport-secondary");
    transport.append(&shuffle);
    transport.append(&repeat);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    transport.append(&spacer);
    let volume_icon = gtk::Image::from_icon_name("audio-volume-medium-symbolic");
    transport.append(&volume_icon);
    let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
    volume.set_draw_value(false);
    volume.set_value(initial.volume);
    volume.set_width_request(75);
    transport.append(&volume);
    let eq = create_equalizer(&audio, settings.clone());
    transport.append(&eq);
    controls.append(&transport);
    layout.append(&controls);

    // At fullscreen width the actual music folders become a third column,
    // rather than leaving a giant unused grey rectangle beside the queue.
    let library_side_panel = gtk::Box::new(gtk::Orientation::Vertical, 6);
    library_side_panel.add_css_class("playlist-panel");
    library_side_panel.add_css_class("library-side-panel");
    library_side_panel.set_width_request(310);
    library_side_panel.set_hexpand(false);
    library_side_panel.set_vexpand(true);
    let library_title = label(tr("Music", "Музыка"), "library-side-heading");
    library_title.set_margin_start(8);
    library_title.set_margin_top(3);
    library_side_panel.append(&library_title);
    let side_folders_list = gtk::ListBox::new();
    side_folders_list.set_selection_mode(gtk::SelectionMode::None);
    side_folders_list.add_css_class("track-list");
    let side_scroll = gtk::ScrolledWindow::new();
    side_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    side_scroll.set_vexpand(true);
    side_scroll.set_child(Some(&side_folders_list));
    library_side_panel.append(&side_scroll);

    let window = ApplicationWindow::builder()
        .application(app).title("SlateAmp")
        .default_width(initial.window_width.max(360))
        .default_height(initial.window_height.max(220))
        .content(&outer).build();
    window.add_css_class("slate-window");
    let state = Rc::new(SlateAmp {
        window, audio, settings, queue: RefCell::new(queue), stations: RefCell::new(stations),
        folders: RefCell::new(Vec::new()), current: Cell::new(None),
        active_radio: RefCell::new(None), playing: Cell::new(false),
        title, subtitle, status, play_button: play, shuffle_button: shuffle,
        repeat_button: repeat, queue_list,
        queue_indicators: RefCell::new(Vec::new()), stations_list: station_list,
        station_play_buttons: RefCell::new(Vec::new()), folders_list,
        folder_status, stack, preferences_window: RefCell::new(None), position, duration, scrubber,
        scrub_blocked: Cell::new(false), spectrum,
        scan_generation: Cell::new(0), scan_rx: RefCell::new(None), bus_watch: RefCell::new(None),
        mpris_connection: RefCell::new(None), mpris_serial: Cell::new(0), volume_slider: volume.clone(),
        layout, player_column, display_panel: display, playlist_panel: panel,
        controls_panel: controls, library_side_panel, side_folders_list,
        layout_mode: Cell::new(LayoutMode::Compact),
    });
    state.set_theme(initial.theme);
    state.set_shuffle(initial.shuffle);
    state.set_repeat(initial.repeat);
    state.refresh_queue();
    state.refresh_stations();
    state.refresh_folders();
    state.scan_library();

    { let s = state.clone(); open.connect_clicked(move |_| s.open_files()); }
    { let s = state.clone(); add_station.connect_clicked(move |_| s.add_station_dialog()); }
    { let s = state.clone(); preferences.connect_clicked(move |_| s.preferences()); }
    { let s = state.clone(); state.play_button.connect_clicked(move |_| s.toggle_play()); }
    // Capture Space before focused buttons, list rows or sliders consume it.
    // Text editing fields are the only exception; typing spaces should keep working.
    {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let s = state.clone();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            if key != gtk::gdk::Key::space && key != gtk::gdk::Key::KP_Space {
                return glib::Propagation::Proceed;
            }
            if modifiers.intersects(gtk::gdk::ModifierType::CONTROL_MASK
                | gtk::gdk::ModifierType::ALT_MASK
                | gtk::gdk::ModifierType::SUPER_MASK)
            {
                return glib::Propagation::Proceed;
            }
            if let Some(focused) = gtk::prelude::GtkWindowExt::focus(&s.window) {
                if focused.is::<gtk::Entry>() || focused.is::<gtk::TextView>()
                    || focused.is::<gtk::SearchEntry>() || focused.is::<gtk::PasswordEntry>()
                {
                    return glib::Propagation::Proceed;
                }
            }
            s.toggle_play();
            glib::Propagation::Stop
        });
        state.window.add_controller(keys);
    }
    { let s = state.clone(); stop.connect_clicked(move |_| s.stop()); }
    { let s = state.clone(); prev.connect_clicked(move |_| s.previous()); }
    { let s = state.clone(); next.connect_clicked(move |_| s.advance(false)); }
    { let s = state.clone(); state.shuffle_button.connect_clicked(move |_| {
        let next = !s.settings.borrow().shuffle;
        s.set_shuffle(next);
    }); }
    { let s = state.clone(); state.repeat_button.connect_clicked(move |_| {
        let next = match s.settings.borrow().repeat { Repeat::Off => Repeat::All, Repeat::All => Repeat::One, Repeat::One => Repeat::Off };
        s.set_repeat(next);
    }); }
    { let s = state.clone(); volume.connect_value_changed(move |slider| {
        s.audio.set_volume(slider.value());
        s.settings.borrow_mut().volume = slider.value();
        save_settings(&s.settings.borrow());
        s.mpris_changed();
    }); }
    { let s = state.clone(); state.scrubber.connect_change_value(move |_, _, value| {
        if !s.scrub_blocked.get() { s.audio.seek(value as u64); }
        glib::Propagation::Proceed
    }); }
    { let s = state.clone(); state.queue_list.connect_row_activated(move |_, row| s.start_track(row.index() as usize)); }
    { let s = state.clone(); state.folders_list.connect_row_activated(move |_, row| s.play_folder(row.index() as usize)); }
    { let s = state.clone(); state.side_folders_list.connect_row_activated(move |_, row| s.play_folder(row.index() as usize)); }
    { let s = state.clone(); quit.connect_clicked(move |_| {
        s.remember_window_geometry();
        s.persist_queue();
        s.audio.stop();
        if let Some(app) = s.window.application() { app.quit(); }
    }); }
    { let s = state.clone(); state.window.connect_close_request(move |window| {
        s.remember_window_geometry();
        if s.settings.borrow().background_on_close {
            window.hide();
            glib::Propagation::Stop
        } else {
            s.persist_queue();
            s.audio.stop();
            if let Some(app) = s.window.application() { app.quit(); }
            glib::Propagation::Proceed
        }
    }); }
    { let s = state.clone();
        let guard = state.audio.bus().expect("playbin must provide a bus").add_watch_local(move |_, message| {
            s.handle_message(message);
            glib::ControlFlow::Continue
        })?;
        state.bus_watch.replace(Some(guard));
    }
    { let s = state.clone(); glib::timeout_add_local(Duration::from_millis(400), move || {
        s.update_responsive_layout();
        s.remember_window_geometry();
        s.tick();
        s.poll_scan();
        glib::ControlFlow::Continue
    }); }
    state.start_mpris();
    state.window.present();
    Ok(state)
}

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id("io.github.eugenek32.SlateAmp").build();
    app.connect_startup(|_| {
        // Native dialogs and the preferences window follow GNOME's scheme.
        adw::StyleManager::default().set_color_scheme(adw::ColorScheme::Default);
        let css = gtk::CssProvider::new();
        css.load_from_data(include_str!("../assets/style.css"));
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
    });
    app.connect_activate(|app| {
        if let Some(main) = app.windows().into_iter()
            .find_map(|w| w.downcast::<ApplicationWindow>().ok()) {
            main.present();
            return;
        }
        if let Err(err) = build_ui(app) {
            eprintln!("SlateAmp startup failed: {err:#}");
            let window = ApplicationWindow::builder().application(app).title("SlateAmp — Error").default_width(500).default_height(220).build();
            let msg = gtk::Label::new(Some(&format!("Cannot initialize audio engine:\n{err:#}")));
            msg.set_wrap(true);
            msg.set_margin_start(24);
            msg.set_margin_end(24);
            window.set_content(Some(&msg));
            window.present();
        }
    });
    app.run()
}

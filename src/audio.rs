use anyhow::{Context, Result};
use gst::prelude::*;

pub struct Audio {
    playbin: gst::Element,
    equalizer: gst::Element,
}

impl Audio {
    pub fn new() -> Result<Self> {
        gst::init()?;
        let playbin = gst::ElementFactory::make("playbin").build()
            .context("GStreamer playbin is missing (install gst-plugins-base)")?;
        let input = gst::ElementFactory::make("audioconvert").build()?;
        let equalizer = gst::ElementFactory::make("equalizer-10bands").build()
            .context("GStreamer 10-band equalizer is missing (install gst-plugins-good)")?;
        let spectrum = gst::ElementFactory::make("spectrum")
            .property("bands", crate::spectrum::FFT_BANDS as u32)
            .property("threshold", -110i32)
            .property("post-messages", true)
            .property("message-magnitude", true)
            .property("message-phase", false)
            .property("interval", 60_000_000u64)
            .build()
            .context("GStreamer spectrum is missing (install gst-plugins-good)")?;
        let output = gst::ElementFactory::make("audioconvert").build()?;
        let bin = gst::Bin::new();
        bin.add_many([&input, &equalizer, &spectrum, &output])?;
        gst::Element::link_many([&input, &equalizer, &spectrum, &output])?;
        let sink = gst::GhostPad::builder_with_target(
            &input.static_pad("sink").context("Missing audio input pad")?
        )?.name("sink").build();
        let src = gst::GhostPad::builder_with_target(
            &output.static_pad("src").context("Missing audio output pad")?
        )?.name("src").build();
        sink.set_active(true)?;
        src.set_active(true)?;
        bin.add_pad(&sink)?;
        bin.add_pad(&src)?;
        playbin.set_property("audio-filter", &bin);
        playbin.set_property("volume", 0.82f64);
        Ok(Self { playbin, equalizer })
    }

    pub fn play(&self, uri: &str) -> Result<()> {
        self.playbin.set_state(gst::State::Ready)?;
        self.playbin.set_property("uri", uri);
        self.playbin.set_state(gst::State::Playing)?;
        Ok(())
    }
    pub fn pause(&self) -> Result<()> {
        self.playbin.set_state(gst::State::Paused)?;
        Ok(())
    }
    pub fn resume(&self) -> Result<()> {
        self.playbin.set_state(gst::State::Playing)?;
        Ok(())
    }
    pub fn stop(&self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
    pub fn set_volume(&self, volume: f64) {
        self.playbin.set_property("volume", volume.clamp(0.0, 1.0));
    }
    pub fn set_band(&self, index: usize, decibels: f64) {
        if index < 10 {
            self.equalizer.set_property(&format!("band{index}"), decibels.clamp(-24.0, 12.0));
        }
    }
    pub fn position(&self) -> Option<u64> {
        self.playbin.query_position::<gst::ClockTime>().map(|t| t.seconds())
    }
    pub fn duration(&self) -> Option<u64> {
        self.playbin.query_duration::<gst::ClockTime>().map(|t| t.seconds())
    }
    pub fn seek(&self, seconds: u64) {
        let _ = self.playbin.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
            gst::ClockTime::from_seconds(seconds),
        );
    }
    pub fn bus(&self) -> Option<gst::Bus> { self.playbin.bus() }
    pub fn equalizer_element(&self) -> gst::Element { self.equalizer.clone() }
}

impl Drop for Audio {
    fn drop(&mut self) { self.stop(); }
}

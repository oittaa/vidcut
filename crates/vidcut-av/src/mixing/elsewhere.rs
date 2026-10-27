use std::convert::Infallible;

use crate::error::{Error, Result};
use crate::ffmpeg::frame::Video;

pub(crate) struct Mixer(Infallible);

pub(crate) struct Overlay(Infallible);

impl Mixer {
    pub(crate) fn new() -> Result<Self> {
        Err(Error::Invalid("pictures mix on Metal on macOS only".into()))
    }

    pub(crate) fn mix(
        &mut self,
        _under: Option<&Video>,
        _over: Option<&Video>,
        _weight: u32,
    ) -> Result<Option<Video>> {
        match self.0 {}
    }

    pub(crate) fn sum(
        &mut self,
        _a: Option<&Video>,
        _b: Option<&Video>,
        _c: Option<&Video>,
    ) -> Result<Option<Video>> {
        match self.0 {}
    }

    pub(crate) fn burn(
        &mut self,
        _under: Option<&Video>,
        _like: &Video,
        _over: &Overlay,
    ) -> Result<Option<Video>> {
        match self.0 {}
    }

    pub(crate) fn place(
        &mut self,
        _under: Option<&Video>,
        _like: &Video,
        _layer: &crate::export::place::Layer,
        _size: (u32, u32),
    ) -> Result<Option<Video>> {
        match self.0 {}
    }
}

impl Overlay {
    pub(crate) fn new(
        mixer: &Mixer,
        _like: &Video,
        _sprites: &[crate::burn::Sprite],
        _shift: crate::burn::Placing,
    ) -> Result<Self> {
        match mixer.0 {}
    }

    pub(crate) fn of(
        mixer: &Mixer,
        _like: &Video,
        _layer: &crate::export::place::Layer,
        _size: (u32, u32),
    ) -> Result<Self> {
        match mixer.0 {}
    }
}

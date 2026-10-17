use crate::ffmpeg;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{context}: {source}")]
    Ffmpeg {
        context: String,
        #[source]
        source: ffmpeg::Error,
    },
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Missing(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait Context<T> {
    fn ctx(self, context: impl FnOnce() -> String) -> Result<T>;
}

impl<T> Context<T> for std::result::Result<T, ffmpeg::Error> {
    fn ctx(self, context: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|source| Error::Ffmpeg {
            context: context(),
            source,
        })
    }
}

use std::path::{Path, PathBuf};

use crate::request::{SpeakerModel, SpeechModel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct File {
    pub name: &'static str,
    pub title: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Hard,
    Cap,
    Soft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    AudioSecs,
    RequestBytes,
    Speakers,
    Channels,
    Terms,
    TermChars,
    GpuGb,
    Scale,
    ChunkFrames,
    HdrPictures,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Doc(&'static str),
    Vidcut,
    Unrecorded,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limit {
    pub measure: Measure,
    pub most: f64,
    pub kind: Kind,
    pub source: Source,
    pub checked: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Service {
    pub url: &'static str,
    pub model: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub dollars_per_hour: f64,
    pub checked: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
    pub title: &'static str,
    pub files: &'static [File],
    pub service: Option<Service>,
    pub languages: &'static [&'static str],
    pub limits: &'static [Limit],
    pub price: Option<Price>,
}

impl Model {
    pub fn most(&self, measure: Measure) -> Option<f64> {
        self.limits
            .iter()
            .find(|l| l.measure == measure)
            .map(|l| l.most)
    }
}

const fn limit(measure: Measure, most: f64, kind: Kind, source: Source) -> Limit {
    Limit {
        measure,
        most,
        kind,
        source,
        checked: None,
    }
}

const fn checked(
    measure: Measure,
    most: f64,
    kind: Kind,
    doc: &'static str,
    on: &'static str,
) -> Limit {
    Limit {
        measure,
        most,
        kind,
        source: Source::Doc(doc),
        checked: Some(on),
    }
}

const GEMINI_DOC: &str = "https://ai.google.dev/gemini-api/docs/models/gemini-3.5-transcribe";

pub const PARAKEET_ULTRA_GGUF: File = File {
    name: "parakeet-ultra-Q8_0.gguf",
    title: SpeechModel::ParakeetUltra.title(),
    url: "https://huggingface.co/handy-computer/parakeet-ultra-gguf/resolve/a1539be1b849ea7463b88343e5277b373d0c4ea9/parakeet-ultra-Q8_0.gguf",
    bytes: 740_363_168,
    sha256: "007a59761e9258779f189df396b50d12b83bb5b79e66df4b955c230d2f2a0a59",
};

pub const QWEN3_ASR_GGUF: File = File {
    name: "Qwen3-ASR-1.7B-Q8_0.gguf",
    title: SpeechModel::Qwen3Asr.title(),
    url: "https://huggingface.co/handy-computer/Qwen3-ASR-1.7B-gguf/resolve/0e0a596e3959c3f3f2ab7d443a5b6f0f4a4bba6f/Qwen3-ASR-1.7B-Q8_0.gguf",
    bytes: 2_185_030_624,
    sha256: "9a0d81792dfea2d5f278b8a63deb3ea6e02139ce42c2301f32ea19c4f77526b7",
};

pub const WHISPER_LARGE_V3_GGUF: File = File {
    name: "whisper-large-v3-Q8_0.gguf",
    title: SpeechModel::WhisperLargeV3.title(),
    url: "https://huggingface.co/handy-computer/whisper-large-v3-gguf/resolve/274ad1d1cf680b7ee53cc70f78461394cd3a344b/whisper-large-v3-Q8_0.gguf",
    bytes: 1_668_741_440,
    sha256: "2fa1a5f179f8a511a53e2108db270aa4af3ce08cd976af4180e2854666bb4ba3",
};

pub const SILERO_VAD_GGUF: File = File {
    name: "silero-vad-v6.2-F32.gguf",
    title: "Silero VAD v6.2",
    url: "https://huggingface.co/handy-computer/silero-vad-v6.2-gguf/resolve/a9b2d61c20ae4ddf3644edb34969e8185a485448/silero-vad-v6.2-F32.gguf",
    bytes: 1_241_056,
    sha256: "0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6",
};

pub const SORTFORMER_GGUF: File = File {
    name: "diar_streaming_sortformer_4spk-v2.1-Q8_0.gguf",
    title: "Sortformer speaker detection",
    url: "https://huggingface.co/handy-computer/diar_streaming_sortformer_4spk-v2.1-gguf/resolve/60ebab3ef7cae9869be0abcde13360347e96ba03/diar_streaming_sortformer_4spk-v2.1-Q8_0.gguf",
    bytes: 139_310_336,
    sha256: "a5dacdc650790266c7a362e54e6bf51952015487edaa606c4e11632bc32442a9",
};

pub const MOSS_GGUF: File = File {
    name: "MOSS-Transcribe-Diarize-Q8_0.gguf",
    title: "MOSS speaker detection",
    url: "https://huggingface.co/handy-computer/moss-transcribe-diarize-gguf/resolve/8c603aa6da33a068e39bbd109786f85786a21834/MOSS-Transcribe-Diarize-Q8_0.gguf",
    bytes: 986_900_160,
    sha256: "2f9039f1f06c7795de78f45ec1abea2712e881eb0f5c7ff8a080e66e1a3ce015",
};

pub static PARAKEET_ULTRA: Model = Model {
    title: SpeechModel::ParakeetUltra.title(),
    files: &[PARAKEET_ULTRA_GGUF],
    service: None,
    languages: &[
        "en", "de", "fr", "es", "it", "pt", "ru", "uk", "hr", "sl", "lv", "lt", "et", "fi", "sv",
        "da", "nl", "pl", "cs", "sk", "hu", "ro", "bg", "el", "mt",
    ],
    limits: &[limit(Measure::GpuGb, 2.0, Kind::Soft, Source::Unrecorded)],
    price: None,
};

pub static QWEN3_ASR: Model = Model {
    title: SpeechModel::Qwen3Asr.title(),
    files: &[QWEN3_ASR_GGUF],
    service: None,
    languages: &[
        "zh", "en", "yue", "ar", "de", "fr", "es", "pt", "id", "it", "ko", "ru", "th", "vi", "ja",
        "tr", "hi", "ms", "nl", "sv", "da", "fi", "pl", "cs", "fil", "fa", "el", "ro", "hu", "mk",
    ],
    limits: &[limit(Measure::GpuGb, 8.0, Kind::Soft, Source::Unrecorded)],
    price: None,
};

pub static WHISPER_LARGE_V3: Model = Model {
    title: SpeechModel::WhisperLargeV3.title(),
    files: &[WHISPER_LARGE_V3_GGUF],
    service: None,
    languages: &[
        "af", "am", "ar", "as", "az", "ba", "be", "bg", "bn", "bo", "br", "bs", "ca", "cs", "cy",
        "da", "de", "el", "en", "es", "et", "eu", "fa", "fi", "fo", "fr", "gl", "gu", "haw", "ha",
        "he", "hi", "hr", "ht", "hu", "hy", "id", "is", "it", "ja", "jw", "ka", "kk", "km", "kn",
        "ko", "la", "lb", "ln", "lo", "lt", "lv", "mg", "mi", "mk", "ml", "mn", "mr", "ms", "mt",
        "my", "ne", "nl", "nn", "no", "oc", "pa", "pl", "ps", "pt", "ro", "ru", "sa", "sd", "si",
        "sk", "sl", "sn", "so", "sq", "sr", "su", "sv", "sw", "ta", "te", "tg", "th", "tk", "tl",
        "tr", "tt", "uk", "ur", "uz", "vi", "yi", "yo", "yue", "zh",
    ],
    limits: &[limit(Measure::GpuGb, 4.0, Kind::Soft, Source::Unrecorded)],
    price: None,
};

pub static SILERO_VAD: Model = Model {
    title: "Silero VAD v6.2",
    files: &[SILERO_VAD_GGUF],
    service: None,
    languages: &[],
    limits: &[],
    price: None,
};

pub static SORTFORMER: Model = Model {
    title: "Sortformer speaker detection",
    files: &[SORTFORMER_GGUF],
    service: None,
    languages: &[],
    limits: &[limit(
        Measure::Speakers,
        4.0,
        Kind::Cap,
        Source::Doc("https://huggingface.co/nvidia/diar_streaming_sortformer_4spk-v2.1"),
    )],
    price: None,
};

pub static MOSS: Model = Model {
    title: "MOSS speaker detection",
    files: &[MOSS_GGUF],
    service: None,
    languages: &["en", "zh"],
    limits: &[],
    price: None,
};

pub const XAI_LANGUAGES: [&str; 38] = [
    "ar", "bs", "bg", "yue", "ca", "zh", "hr", "cs", "da", "nl", "en", "fil", "fi", "fr", "de",
    "el", "hi", "hu", "id", "it", "ja", "ko", "mk", "ms", "nb", "fa", "pl", "pt", "ro", "ru", "sk",
    "es", "sv", "th", "tr", "uk", "ur", "vi",
];

pub const GOOGLE_LANGUAGES: [&str; 83] = [
    "en-US",
    "es-US",
    "pt-BR",
    "cmn-Hans-CN",
    "yue-Hant-HK",
    "bn-BD",
    "pa-IN",
    "af-ZA",
    "am-ET",
    "ar-EG",
    "hy-AM",
    "as-IN",
    "az-AZ",
    "be-BY",
    "bn-IN",
    "bs-BA",
    "bg-BG",
    "rup-BG",
    "my-MM",
    "ca-ES",
    "ceb",
    "km-KH",
    "hr-HR",
    "cs-CZ",
    "da-DK",
    "nl-NL",
    "en-GB",
    "en-IN",
    "et-EE",
    "fa-IR",
    "fil-PH",
    "fi-FI",
    "fr-FR",
    "gl-ES",
    "ka-GE",
    "de-DE",
    "el-GR",
    "gu-IN",
    "ha-NG",
    "he-IL",
    "hi-IN",
    "hu-HU",
    "is-IS",
    "id-ID",
    "it-IT",
    "ja-JP",
    "jv-ID",
    "kea-CV",
    "kn-IN",
    "kk-KZ",
    "ko-KR",
    "ky-KG",
    "lv-LV",
    "ln-CD",
    "lt-LT",
    "mk-MK",
    "ms-MY",
    "ml-IN",
    "mt-MT",
    "mr-IN",
    "mn-MN",
    "ne-NP",
    "nb-NO",
    "or-IN",
    "pl-PL",
    "pt-PT",
    "pa-Guru-IN",
    "ro-RO",
    "ru-RU",
    "sr-RS",
    "sd-Arab-IN",
    "sk-SK",
    "sl-SI",
    "es-419",
    "sw-KE",
    "sv-SE",
    "tg-TJ",
    "te-IN",
    "th-TH",
    "tr-TR",
    "uk-UA",
    "uz-UZ",
    "vi-VN",
];

pub const XAI_SERVICE: Service = Service {
    url: "https://api.x.ai/v1/stt",
    model: "grok-voice-transcribe-2.0",
};

pub const XAI_REQUEST_BYTES: u64 = 500_000_000;

pub const GOOGLE_SERVICE: Service = Service {
    url: "https://generativelanguage.googleapis.com",
    model: "gemini-3.5-transcribe",
};

pub static XAI: Model = Model {
    title: SpeechModel::Xai.title(),
    files: &[],
    service: Some(XAI_SERVICE),
    languages: &XAI_LANGUAGES,
    limits: &[
        limit(
            Measure::RequestBytes,
            XAI_REQUEST_BYTES as f64,
            Kind::Hard,
            Source::Unrecorded,
        ),
        limit(Measure::Terms, 100.0, Kind::Hard, Source::Unrecorded),
        limit(Measure::TermChars, 50.0, Kind::Hard, Source::Unrecorded),
        limit(Measure::Channels, 8.0, Kind::Hard, Source::Unrecorded),
    ],
    price: Some(Price {
        dollars_per_hour: 0.10,
        checked: "2026-10",
    }),
};

pub static GOOGLE: Model = Model {
    title: SpeechModel::Google.title(),
    files: &[],
    service: Some(GOOGLE_SERVICE),
    languages: &GOOGLE_LANGUAGES,
    limits: &[
        checked(
            Measure::AudioSecs,
            1800.0,
            Kind::Hard,
            GEMINI_DOC,
            "2026-10-24",
        ),
        checked(Measure::Speakers, 8.0, Kind::Soft, GEMINI_DOC, "2026-10-24"),
    ],
    price: Some(Price {
        dollars_per_hour: 0.30,
        checked: "2026-10",
    }),
};

pub const DPDFNET_ONNX: File = File {
    name: "onnx/dpdfnet2_48khz_hr.onnx",
    title: "DPDFNet voice isolation",
    url: "https://huggingface.co/Ceva-IP/DPDFNet/resolve/dd6818d00f50c836fed43a6243ebe49116de5964/onnx/dpdfnet2_48khz_hr.onnx",
    bytes: 10_493_337,
    sha256: "7f0575a5cec0ba4ffd8f8bd657e06d007e4ccdd955d76faab922b9d3291dc14b",
};

pub static DPDFNET: Model = Model {
    title: "DPDFNet voice isolation",
    files: &[DPDFNET_ONNX],
    service: None,
    languages: &[],
    limits: &[],
    price: None,
};

pub const NANOVSR_SCALE: u32 = 4;
pub const NANOVSR_FRAMES: usize = 15;

pub const NANOVSR_WEIGHTS: File = File {
    name: "nanovsr/nanovsr_5.4m.pth",
    title: "NanoVSR 5.4M upscaler",
    url: "https://github.com/filippawlicki/nanovsr/releases/download/v1.0/nanovsr_5.4m.pth",
    bytes: 24_624_333,
    sha256: "42a53e1dc9b928947ea9cea85d7dd6f7bd82b325e92374dc1946d2cbcadf31bc",
};

pub static NANOVSR: Model = Model {
    title: "NanoVSR 5.4M",
    files: &[NANOVSR_WEIGHTS],
    service: None,
    languages: &[],
    limits: &[
        limit(
            Measure::Scale,
            NANOVSR_SCALE as f64,
            Kind::Hard,
            Source::Unrecorded,
        ),
        limit(
            Measure::ChunkFrames,
            NANOVSR_FRAMES as f64,
            Kind::Hard,
            Source::Vidcut,
        ),
        limit(Measure::HdrPictures, 0.0, Kind::Hard, Source::Vidcut),
    ],
    price: None,
};

pub const FLASHVSR_SCALE: u32 = 4;

pub const FLASHVSR_FILES: [File; 4] = [
    File {
        name: "flashvsr-plus/turbo/dit.safetensors",
        title: "FlashVSR+ Turbo transformer",
        url: "https://huggingface.co/ruchangyao/FlashVSR_plus/resolve/46b79520946ac67dec94d1566490690953c7debc/turbo/dit.safetensors",
        bytes: 5_676_070_392,
        sha256: "7862b92178176fa3aebc34c6c071daca6b56ab1dddb8e7ff9a01c47f5226ec8b",
    },
    File {
        name: "flashvsr-plus/turbo/lq_proj_in.safetensors",
        title: "FlashVSR+ Turbo input projection",
        url: "https://huggingface.co/ruchangyao/FlashVSR_plus/resolve/46b79520946ac67dec94d1566490690953c7debc/turbo/lq_proj_in.safetensors",
        bytes: 1_151_384_272,
        sha256: "e77b378ae12342eefbb58f924256f362a151b93ce73dd32808fcdaca9c051496",
    },
    File {
        name: "flashvsr-plus/TCDecoder.ckpt",
        title: "FlashVSR+ decoder",
        url: "https://huggingface.co/ruchangyao/FlashVSR_plus/resolve/46b79520946ac67dec94d1566490690953c7debc/TCDecoder.ckpt",
        bytes: 189_018_333,
        sha256: "e224bdcf2f52745cbf4d393ff5374c2ba09e90285d5d19062d2bf63b915b6161",
    },
    File {
        name: "flashvsr-plus/prompt_tensor/posi_prompt.pth",
        title: "FlashVSR+ prompt",
        url: "https://raw.githubusercontent.com/OpenImagingLab/FlashVSR_Plus/b548a2cc2c929d3d4676f2973abe27876ec4dee7/prompt_tensor/posi_prompt.pth",
        bytes: 4_195_504,
        sha256: "4601107a11e4e11a936a6b79df579e54dbc99872132bf542151f0ffd65b4b1ef",
    },
];

pub static FLASHVSR: Model = Model {
    title: "FlashVSR+ Turbo",
    files: &FLASHVSR_FILES,
    service: None,
    languages: &[],
    limits: &[
        limit(
            Measure::Scale,
            FLASHVSR_SCALE as f64,
            Kind::Hard,
            Source::Unrecorded,
        ),
        limit(Measure::HdrPictures, 0.0, Kind::Hard, Source::Vidcut),
    ],
    price: None,
};

pub static ALL: [&Model; 11] = [
    &PARAKEET_ULTRA,
    &QWEN3_ASR,
    &WHISPER_LARGE_V3,
    &SILERO_VAD,
    &SORTFORMER,
    &MOSS,
    &XAI,
    &GOOGLE,
    &DPDFNET,
    &NANOVSR,
    &FLASHVSR,
];

pub fn speech(model: SpeechModel) -> &'static Model {
    match model {
        SpeechModel::ParakeetUltra => &PARAKEET_ULTRA,
        SpeechModel::Qwen3Asr => &QWEN3_ASR,
        SpeechModel::WhisperLargeV3 => &WHISPER_LARGE_V3,
        SpeechModel::Xai => &XAI,
        SpeechModel::Google => &GOOGLE,
    }
}

pub fn speakers(model: SpeakerModel) -> Option<&'static Model> {
    match model {
        SpeakerModel::Auto => None,
        SpeakerModel::Moss => Some(&MOSS),
        SpeakerModel::Sortformer => Some(&SORTFORMER),
    }
}

pub fn holding(f: &File) -> Option<&'static Model> {
    ALL.iter()
        .copied()
        .find(|m| m.files.iter().any(|g| g.name == f.name))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pieces {
    pub untimed_secs: f64,
    pub shortest_secs: f64,
    pub longest_secs: f64,
    pub qwen3_base_mb: f64,
    pub qwen3_mb_per_second: f64,
}

pub const PIECES: Pieces = Pieces {
    untimed_secs: 10.0,
    shortest_secs: 30.0,
    longest_secs: 300.0,
    qwen3_base_mb: 3300.0,
    qwen3_mb_per_second: 21.0,
};

pub const TRANSCRIBE_CPP: &str = "0.4.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Runtime {
    pub lane: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub library: &'static str,
}

impl Runtime {
    pub fn archive(&self) -> String {
        format!("transcribe-native-{TRANSCRIBE_CPP}-{}.tar.gz", self.lane)
    }

    pub fn url(&self) -> String {
        format!(
            "https://github.com/handy-computer/transcribe.cpp/releases/download/v{TRANSCRIBE_CPP}/{}",
            self.archive()
        )
    }

    pub fn home(&self, dir: &Path) -> PathBuf {
        dir.join(format!("transcribe-{TRANSCRIBE_CPP}"))
    }

    pub fn present(&self, dir: &Path) -> bool {
        self.home(dir).join(self.library).is_file()
    }
}

pub const RUNTIMES: [Runtime; 5] = [
    Runtime {
        lane: "windows-x86_64-cpu-vulkan",
        bytes: 18_052_295,
        sha256: "272eb97795048ccb557c85bb9dd44c9e530e1c928443114c39816a8a5f76269d",
        library: "transcribe.dll",
    },
    Runtime {
        lane: "macos-arm64-metal",
        bytes: 2_491_746,
        sha256: "7369e36dc2a5fc5d52eee5766fa03ad22d3f533c8688bf1deccf3ffdc202ea24",
        library: "libtranscribe.dylib",
    },
    Runtime {
        lane: "macos-x86_64-cpu",
        bytes: 1_619_231,
        sha256: "513a3976a2f0ebc0666c9670551e96c226e39ed14090a51f2efa0874f41258d8",
        library: "libtranscribe.dylib",
    },
    Runtime {
        lane: "linux-x86_64-cpu-vulkan",
        bytes: 22_561_707,
        sha256: "4b58e80c63d7f2b531b4e844b94c0703593f48a5be6afd829389cdda14a13edf",
        library: "libtranscribe.so",
    },
    Runtime {
        lane: "linux-aarch64-cpu-vulkan",
        bytes: 18_137_415,
        sha256: "3671d8a07e256642d6bb38cc05ad8bbe88c76f814aeb704d1f8b57c61010db02",
        library: "libtranscribe.so",
    },
];

pub const RUNTIME: Option<Runtime> = if cfg!(all(windows, target_arch = "x86_64")) {
    Some(RUNTIMES[0])
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    Some(RUNTIMES[1])
} else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
    Some(RUNTIMES[2])
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some(RUNTIMES[3])
} else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
    Some(RUNTIMES[4])
} else {
    None
};

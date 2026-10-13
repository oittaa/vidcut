//! The computer's GPUs and the choice of one (`vidcut_av::gpus`). A test
//! binary of its own: the choice is the process's.
#![cfg(feature = "gpu-tests")]

use vidcut_av::export::encode;
use vidcut_av::gpus;
use vidcut_av::hw::{GpuDecoder, Kind};

/// Every GPU listed has an id with its PCI slot; choosing one moves the
/// decoder there, NVIDIA's to NVDEC and the others' to their own, and an
/// empty choice or one not found leaves the automatic one.
#[test]
fn the_chosen_gpu_decodes() {
    let gpus = gpus::list();
    for g in gpus {
        eprintln!("{} {} ({})", g.id, g.name, g.vendor);
        assert!(g.id.contains('@'), "{}: no PCI slot", g.id);
    }
    let Some(automatic) = gpus::automatic() else {
        eprintln!("skipping: no GPU here");
        return;
    };
    let kind = |g: &gpus::Gpu| {
        if g.vendor == "NVIDIA" {
            Kind::Nvdec
        } else if cfg!(windows) {
            Kind::D3d11va
        } else {
            Kind::Vaapi
        }
    };
    // Encoders tested on every GPU; exports take the chosen one's.
    let detected = encode::detect();
    for g in gpus {
        gpus::choose(&g.id);
        assert_eq!(gpus::chosen().as_ref(), Some(g));
        let d = GpuDecoder::best().expect("a decoder on the chosen GPU");
        assert_eq!(d.kind(), kind(g), "{}", g.name);
        let own = detected.in_use();
        eprintln!(
            "{}: {:?}",
            g.name,
            own.encoders
                .iter()
                .filter(|e| e.ok)
                .map(|e| &e.encoder)
                .collect::<Vec<_>>()
        );
        assert!(!own.encoders.is_empty(), "{}: no encoders tested", g.name);
        assert!(
            own.encoders.iter().all(|e| g.encodes_with(e.kind)),
            "{}: another GPU's encoders",
            g.name
        );
    }
    for none in ["", "10de:ffff@ff:00.0"] {
        gpus::choose(none);
        assert_eq!(gpus::in_use().as_ref(), Some(&automatic));
        let d = GpuDecoder::best().expect("the automatic decoder");
        assert_eq!(d.kind(), kind(&automatic));
    }
}

//! The computer's GPUs, for the preferences to choose one (`gpu.device`).
//! Each is known by its PCI vendor and device ids and its PCI slot
//! (`10de:2702@01:00.0`), which the firmware gives it before any system
//! starts: the same on Linux and Windows on one computer, so one choice
//! serves both. A card moved to another slot is still found by its ids
//! while it is the only one of its model.
//!
//! The GPU chosen ([`choose`]) decodes (`GpuDecoder::best`), holds the CUDA
//! device (`CudaDevice::open`) and encodes (`Availability::on`). Without a
//! choice, or while the chosen one isn't there, the best the computer has
//! does ([`automatic`]).

use std::sync::{Mutex, OnceLock};

use serde::Serialize;

/// PCI vendor ids.
pub const NVIDIA: u32 = 0x10de;
pub const AMD: u32 = 0x1002;
pub const INTEL: u32 = 0x8086;

/// A GPU of this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Gpu {
    /// What the preferences name it by: its PCI vendor and device ids and
    /// its slot (bus, device, function), in hex: `10de:2702@01:00.0`; the
    /// ids alone where the slot isn't known.
    pub id: String,
    /// Its name, as its driver gives it.
    pub name: String,
    /// Its maker: `NVIDIA`, `AMD`, `Intel`, or the PCI vendor id.
    pub vendor: String,
    #[serde(skip)]
    pub(crate) pci_vendor: u32,
    #[serde(skip)]
    pub(crate) slot: Option<Slot>,
    /// Its CUDA device's index (NVIDIA's).
    #[serde(skip)]
    pub(crate) cuda: Option<i32>,
    /// Its Direct3D 11 adapter's index (Windows).
    #[serde(skip)]
    pub(crate) adapter: Option<u32>,
    /// Its render node (Linux).
    #[serde(skip)]
    pub(crate) node: Option<String>,
}

/// A PCI slot: bus, device, function.
pub(crate) type Slot = (u32, u32, u32);

/// The id of a GPU with PCI ids `vendor` and `device` in `slot`.
fn id(vendor: u32, device: u32, slot: Option<Slot>) -> String {
    match slot {
        Some((b, d, f)) => format!("{vendor:04x}:{device:04x}@{b:02x}:{d:02x}.{f:x}"),
        None => format!("{vendor:04x}:{device:04x}"),
    }
}

/// The maker of PCI vendor `vendor`, for people.
fn maker(vendor: u32) -> String {
    match vendor {
        NVIDIA => "NVIDIA".into(),
        AMD => "AMD".into(),
        INTEL => "Intel".into(),
        v => format!("{v:04x}"),
    }
}

/// The slot in a PCI address as Linux and CUDA write it,
/// `[domain:]bus:device.function` in hex (`0000:01:00.0`).
pub(crate) fn parse_slot(address: &str) -> Option<Slot> {
    let mut parts = address.trim().rsplit(':');
    let (device, function) = parts.next()?.split_once('.')?;
    let bus = parts.next()?;
    let hex = |s: &str| u32::from_str_radix(s, 16).ok();
    Some((hex(bus)?, hex(device)?, hex(function)?))
}

/// The GPU among `gpus` that `id` names: the one with that id, else the
/// only one of its model (moved to another slot). `None` when it isn't
/// there, or its model is more than one GPU and none has its slot.
pub fn find<'a>(gpus: &'a [Gpu], id: &str) -> Option<&'a Gpu> {
    if let Some(g) = gpus.iter().find(|g| g.id == id) {
        return Some(g);
    }
    let model = id.split('@').next()?;
    let mut same = gpus
        .iter()
        .filter(|g| g.id.split('@').next() == Some(model));
    match (same.next(), same.next()) {
        (Some(g), None) => Some(g),
        _ => None,
    }
}

/// The computer's GPUs, listed once: in Windows' adapter order, or the
/// render nodes' on Linux. None listed on macOS, which has no choice.
pub fn list() -> &'static [Gpu] {
    static LIST: OnceLock<Vec<Gpu>> = OnceLock::new();
    LIST.get_or_init(listed)
}

/// The GPU that works when none is chosen, or while the chosen one isn't
/// there: NVIDIA's that CUDA lists first, else the first other one (on
/// Windows the main display's adapter, as a rule).
pub fn automatic() -> Option<Gpu> {
    let gpus = list();
    gpus.iter()
        .find(|g| g.cuda == Some(0))
        .or_else(|| gpus.iter().find(|g| g.pci_vendor != NVIDIA))
        .cloned()
}

/// The id the preferences chose; empty for none.
static CHOSEN: Mutex<String> = Mutex::new(String::new());

/// Decodes and encodes on the GPU `id` names ([`Gpu::id`]) from now on;
/// empty: on the [`automatic`] one.
pub fn choose(id: &str) {
    let mut chosen = CHOSEN.lock().unwrap_or_else(|e| e.into_inner());
    if *chosen != id {
        *chosen = id.to_string();
    }
}

/// The id chosen ([`choose`]); empty for none.
pub fn chosen_id() -> String {
    CHOSEN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The GPU chosen, while this computer has it.
pub fn chosen() -> Option<Gpu> {
    let id = chosen_id();
    if id.is_empty() {
        return None;
    }
    find(list(), &id).cloned()
}

/// The GPU that works: the chosen one, else the [`automatic`] one.
pub fn in_use() -> Option<Gpu> {
    chosen().or_else(automatic)
}

impl Gpu {
    /// Whether it has encoders of `kind`: NVENC on NVIDIA's, AMF on AMD's
    /// (Windows), QSV on Intel's, VAAPI on the others' render nodes (Linux),
    /// VideoToolbox on a Mac's.
    pub fn encodes_with(&self, kind: vidcut_core::export::EncoderChoice) -> bool {
        use vidcut_core::export::EncoderChoice as E;
        match kind {
            E::Auto | E::Cpu => true,
            E::Nvenc => self.pci_vendor == NVIDIA,
            E::Amf => cfg!(windows) && self.pci_vendor == AMD,
            E::Qsv => self.pci_vendor == INTEL,
            E::Vaapi => cfg!(target_os = "linux") && self.pci_vendor != NVIDIA,
            E::VideoToolbox => cfg!(target_os = "macos"),
        }
    }

    /// Whether `status`, an encoder detection tested, ran on it: NVENC on
    /// NVIDIA's, the others on its adapter or render node.
    pub(crate) fn ran(&self, status: &crate::export::encode::EncoderStatus) -> bool {
        use vidcut_core::export::EncoderChoice as E;
        let Some(on) = status.device.as_deref() else {
            return status.kind == E::VideoToolbox;
        };
        match status.kind {
            E::Nvenc => self.cuda.is_some_and(|c| c.to_string() == on),
            E::VideoToolbox => true,
            _ => {
                self.node.as_deref() == Some(on)
                    || self.adapter.is_some_and(|a| a.to_string() == on)
            }
        }
    }
}

#[cfg(windows)]
fn listed() -> Vec<Gpu> {
    use ::windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
    };
    /// Microsoft's Basic Render Driver: no GPU behind it.
    const MICROSOFT: u32 = 0x1414;
    let cuda = crate::cuda::listed();
    // SAFETY: DXGI's factory and adapters, as Direct3D 11 is opened on
    // them by their index (FFmpeg enumerates them the same).
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut gpus = Vec::new();
    for i in 0.. {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else {
            break;
        };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || desc.VendorId == MICROSOFT {
            continue;
        }
        let slot = windows_slot(desc.AdapterLuid);
        let name = String::from_utf16_lossy(&desc.Description);
        let name = name.trim_end_matches('\0').trim().to_string();
        gpus.push(Gpu {
            id: id(desc.VendorId, desc.DeviceId, slot),
            name,
            vendor: maker(desc.VendorId),
            pci_vendor: desc.VendorId,
            slot,
            cuda: None,
            adapter: Some(i),
            node: None,
        });
    }
    with_cuda(&mut gpus, &cuda);
    gpus
}

/// The PCI slot of the adapter `luid` names, as its kernel driver says.
#[cfg(windows)]
fn windows_slot(luid: ::windows::Win32::Foundation::LUID) -> Option<Slot> {
    use ::windows::Wdk::Graphics::Direct3D::{
        D3DKMT_ADAPTERADDRESS, D3DKMT_CLOSEADAPTER, D3DKMT_OPENADAPTERFROMLUID,
        D3DKMT_QUERYADAPTERINFO, D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid,
        D3DKMTQueryAdapterInfo, KMTQAITYPE_ADAPTERADDRESS,
    };
    let mut open = D3DKMT_OPENADAPTERFROMLUID {
        AdapterLuid: luid,
        hAdapter: 0,
    };
    // SAFETY: the adapter opened by its LUID, asked for its address into a
    // structure of the size given, and closed.
    unsafe {
        if D3DKMTOpenAdapterFromLuid(&mut open).is_err() {
            return None;
        }
        let mut address = D3DKMT_ADAPTERADDRESS::default();
        let mut query = D3DKMT_QUERYADAPTERINFO {
            hAdapter: open.hAdapter,
            Type: KMTQAITYPE_ADAPTERADDRESS,
            pPrivateDriverData: (&raw mut address).cast(),
            PrivateDriverDataSize: size_of::<D3DKMT_ADAPTERADDRESS>() as u32,
        };
        let asked = D3DKMTQueryAdapterInfo(&mut query);
        let _ = D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER {
            hAdapter: open.hAdapter,
        });
        asked.is_ok().then_some((
            address.BusNumber,
            address.DeviceNumber,
            address.FunctionNumber,
        ))
    }
}

#[cfg(target_os = "linux")]
fn listed() -> Vec<Gpu> {
    use std::path::Path;
    let cuda = crate::cuda::listed();
    let names = pci_names();
    let mut gpus = Vec::new();
    for (node, _driver) in crate::export::encode::render_nodes() {
        let name = node.trim_start_matches("/dev/dri/");
        let dev = Path::new("/sys/class/drm").join(name).join("device");
        let hex = |file: &str| {
            let text = std::fs::read_to_string(dev.join(file)).ok()?;
            u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
        };
        let (Some(vendor), Some(device)) = (hex("vendor"), hex("device")) else {
            continue;
        };
        let slot = std::fs::canonicalize(&dev)
            .ok()
            .and_then(|p| parse_slot(&p.file_name()?.to_string_lossy()));
        let named = std::fs::read_to_string(dev.join("product_name"))
            .ok()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .or_else(|| names.get(&(vendor, device)).cloned())
            .unwrap_or_else(|| format!("{} GPU {vendor:04x}:{device:04x}", maker(vendor)));
        gpus.push(Gpu {
            id: id(vendor, device, slot),
            name: named,
            vendor: maker(vendor),
            pci_vendor: vendor,
            slot,
            cuda: None,
            adapter: None,
            node: Some(node),
        });
    }
    with_cuda(&mut gpus, &cuda);
    gpus
}

/// GPU names from the PCI ids database (`pci.ids`, which most systems
/// have): its vendor's and device's names, by their ids.
#[cfg(target_os = "linux")]
fn pci_names() -> std::collections::HashMap<(u32, u32), String> {
    let mut names = std::collections::HashMap::new();
    let Some(text) = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
    else {
        return names;
    };
    // Vendor lines (`10de  NVIDIA Corporation`), each followed by its
    // devices' lines, a tab in (`2702  AD103 [GeForce RTX 4080 SUPER]`).
    let mut vendor = None;
    for line in text.lines() {
        if line.starts_with('#') || line.starts_with("\t\t") || line.is_empty() {
            continue;
        }
        let entry = line.trim_start_matches('\t').split_once("  ");
        let id = entry.and_then(|(id, _)| u32::from_str_radix(id, 16).ok());
        if !line.starts_with('\t') {
            vendor = id.filter(|v| [NVIDIA, AMD, INTEL].contains(v));
        } else if let (Some(v), Some(id), Some((_, name))) = (vendor, id, entry) {
            names.insert((v, id), format!("{} {}", maker(v), name.trim()));
        }
    }
    names
}

/// CUDA's GPUs alone, which no Mac has.
#[cfg(not(any(windows, target_os = "linux")))]
fn listed() -> Vec<Gpu> {
    let mut gpus = Vec::new();
    with_cuda(&mut gpus, &crate::cuda::listed());
    gpus
}

/// Gives NVIDIA's GPUs their CUDA devices, by their slots, and CUDA's
/// names for them. Without slots, the one NVIDIA GPU is the one CUDA
/// device.
fn with_cuda(gpus: &mut Vec<Gpu>, cuda: &[crate::cuda::Listed]) {
    let nvidia = gpus.iter().filter(|g| g.pci_vendor == NVIDIA).count();
    for g in gpus.iter_mut().filter(|g| g.pci_vendor == NVIDIA) {
        let found = match g.slot {
            Some(s) => cuda.iter().find(|c| c.slot == Some(s)),
            None if nvidia == 1 && cuda.len() == 1 => cuda.first(),
            None => None,
        };
        if let Some(c) = found {
            g.cuda = Some(c.index);
            g.name = c.name.clone();
        }
    }
    // CUDA's GPUs no system lists (compute-only cards).
    for c in cuda {
        if gpus.iter().any(|g| g.cuda == Some(c.index)) {
            continue;
        }
        let device = c.slot.and_then(pci_device).unwrap_or(0);
        gpus.push(Gpu {
            id: id(NVIDIA, device, c.slot),
            name: c.name.clone(),
            vendor: maker(NVIDIA),
            pci_vendor: NVIDIA,
            slot: c.slot,
            cuda: Some(c.index),
            adapter: None,
            node: None,
        });
    }
}

/// The PCI device id of the card in `slot`, as Linux's sysfs has it.
fn pci_device((bus, device, function): Slot) -> Option<u32> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let path = format!("/sys/bus/pci/devices/0000:{bus:02x}:{device:02x}.{function:x}/device");
    let text = std::fs::read_to_string(path).ok()?;
    u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
}

#[cfg(test)]
mod tests {
    use super::{Gpu, NVIDIA, find, id, parse_slot};

    fn gpu(vendor: u32, device: u32, slot: Option<(u32, u32, u32)>) -> Gpu {
        Gpu {
            id: id(vendor, device, slot),
            name: String::new(),
            vendor: String::new(),
            pci_vendor: vendor,
            slot,
            cuda: None,
            adapter: None,
            node: None,
        }
    }

    #[test]
    fn ids_are_pci_ids_and_slot() {
        assert_eq!(id(NVIDIA, 0x2702, Some((1, 0, 0))), "10de:2702@01:00.0");
        assert_eq!(id(0x1002, 0x13c0, Some((0x7c, 0, 0))), "1002:13c0@7c:00.0");
        assert_eq!(id(0x8086, 0x46d4, None), "8086:46d4");
        assert_eq!(parse_slot("0000:01:00.0"), Some((1, 0, 0)));
        assert_eq!(parse_slot("00000000:65:00.1"), Some((0x65, 0, 1)));
        assert_eq!(parse_slot("01:00.0"), Some((1, 0, 0)));
        assert_eq!(parse_slot("platform"), None);
    }

    /// A GPU is found by its id; moved to another slot, by its model while
    /// it is the only one of it.
    #[test]
    fn a_chosen_gpu_is_found_by_its_slot_or_its_model() {
        let nvidia = gpu(NVIDIA, 0x2702, Some((1, 0, 0)));
        let radeon = gpu(0x1002, 0x13c0, Some((0x7c, 0, 0)));
        let gpus = [nvidia.clone(), radeon.clone()];
        assert_eq!(find(&gpus, "10de:2702@01:00.0"), Some(&nvidia));
        assert_eq!(find(&gpus, "10de:2702@02:00.0"), Some(&nvidia));
        assert_eq!(find(&gpus, "10de:2702"), Some(&nvidia));
        assert_eq!(find(&gpus, "10de:2704@01:00.0"), None);
        assert_eq!(find(&gpus, ""), None);
        // Two of a model: only the slot tells them apart.
        let twins = [
            gpu(NVIDIA, 0x2702, Some((1, 0, 0))),
            gpu(NVIDIA, 0x2702, Some((2, 0, 0))),
        ];
        assert_eq!(find(&twins, "10de:2702@02:00.0"), Some(&twins[1]));
        assert_eq!(find(&twins, "10de:2702@03:00.0"), None);
    }

    /// An encoder detection tested is a GPU's when it ran there: NVENC on
    /// its CUDA device, the others on its adapter or render node.
    #[test]
    fn encoders_belong_to_the_gpu_they_ran_on() {
        use crate::export::encode::{Availability, EncoderStatus};
        use vidcut_core::export::EncoderChoice as E;
        let status = |encoder: &str, kind, device: &str| EncoderStatus {
            encoder: encoder.into(),
            kind,
            device: Some(device.into()),
            pixel: "nv12".into(),
            ok: true,
            error: None,
        };
        let detected = Availability {
            encoders: vec![
                status("h264_nvenc", E::Nvenc, "0"),
                status("h264_nvenc", E::Nvenc, "1"),
                status("h264_amf", E::Amf, "1"),
                status("h264_vaapi", E::Vaapi, "/dev/dri/renderD129"),
            ],
        };
        let names = |g: &Gpu| {
            detected
                .on(g)
                .encoders
                .iter()
                .map(|e| format!("{} {}", e.encoder, e.device.as_deref().unwrap_or("")))
                .collect::<Vec<_>>()
        };
        let mut second = gpu(NVIDIA, 0x2704, Some((2, 0, 0)));
        second.cuda = Some(1);
        assert_eq!(names(&second), ["h264_nvenc 1"]);
        let mut radeon = gpu(0x1002, 0x13c0, Some((0x0d, 0, 0)));
        radeon.adapter = Some(1);
        assert_eq!(names(&radeon), ["h264_amf 1"]);
        let mut linux = gpu(0x1002, 0x744c, Some((3, 0, 0)));
        linux.node = Some("/dev/dri/renderD129".into());
        assert_eq!(names(&linux), ["h264_vaapi /dev/dri/renderD129"]);
        // What each GPU has, by its maker: NVENC on NVIDIA's alone.
        assert!(second.encodes_with(E::Nvenc) && !second.encodes_with(E::Amf));
        assert!(!radeon.encodes_with(E::Nvenc) && radeon.encodes_with(E::Auto));
        assert_eq!(radeon.encodes_with(E::Amf), cfg!(windows));
        assert_eq!(linux.encodes_with(E::Vaapi), cfg!(target_os = "linux"));
    }
}

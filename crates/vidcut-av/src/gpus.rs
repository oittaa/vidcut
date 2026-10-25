use std::sync::{Mutex, OnceLock};

use serde::Serialize;

pub const NVIDIA: u32 = 0x10de;
pub const AMD: u32 = 0x1002;
pub const INTEL: u32 = 0x8086;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Gpu {
    pub id: String,
    pub name: String,
    pub vendor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
    #[serde(skip)]
    pub(crate) pci_vendor: u32,
    #[serde(skip)]
    pub(crate) slot: Option<Slot>,
    #[serde(skip)]
    pub(crate) cuda: Option<i32>,
    #[serde(skip)]
    pub(crate) uuid: Option<[u8; 16]>,
    #[serde(skip)]
    pub(crate) adapter: Option<u32>,
    #[serde(skip)]
    pub(crate) node: Option<String>,
}

pub(crate) type Slot = (u32, u32, u32);

fn id(vendor: u32, device: u32, slot: Option<Slot>) -> String {
    match slot {
        Some((b, d, f)) => format!("{vendor:04x}:{device:04x}@{b:02x}:{d:02x}.{f:x}"),
        None => format!("{vendor:04x}:{device:04x}"),
    }
}

fn maker(vendor: u32) -> String {
    match vendor {
        NVIDIA => "NVIDIA".into(),
        AMD => "AMD".into(),
        INTEL => "Intel".into(),
        v => format!("{v:04x}"),
    }
}

pub(crate) fn parse_slot(address: &str) -> Option<Slot> {
    let mut parts = address.trim().rsplit(':');
    let (device, function) = parts.next()?.split_once('.')?;
    let bus = parts.next()?;
    let hex = |s: &str| u32::from_str_radix(s, 16).ok();
    Some((hex(bus)?, hex(device)?, hex(function)?))
}

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

pub fn list() -> &'static [Gpu] {
    static LIST: OnceLock<Vec<Gpu>> = OnceLock::new();
    LIST.get_or_init(listed)
}

pub(crate) fn of(vendor: u32, device: u32) -> Option<Gpu> {
    let model = id(vendor, device, None);
    list().iter().find(|g| g.id.starts_with(&model)).cloned()
}

pub fn automatic() -> Option<Gpu> {
    let gpus = list();
    gpus.iter()
        .find(|g| g.cuda == Some(0))
        .or_else(|| gpus.iter().find(|g| g.pci_vendor != NVIDIA))
        .cloned()
}

static CHOSEN: Mutex<String> = Mutex::new(String::new());

pub fn choose(id: &str) {
    let mut chosen = CHOSEN.lock().unwrap_or_else(|e| e.into_inner());
    if *chosen != id {
        *chosen = id.to_string();
    }
}

pub fn chosen_id() -> String {
    CHOSEN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn chosen() -> Option<Gpu> {
    let id = chosen_id();
    if id.is_empty() {
        return None;
    }
    find(list(), &id).cloned()
}

pub fn in_use() -> Option<Gpu> {
    chosen().or_else(automatic)
}

impl Gpu {
    pub fn uuid(&self) -> Option<[u8; 16]> {
        self.uuid
    }

    pub fn encodes_with(&self, kind: vidcut_core::export::EncoderChoice) -> bool {
        use vidcut_core::export::EncoderChoice as E;
        match kind {
            E::Auto | E::Cpu => true,
            E::Nvenc => self.pci_vendor == NVIDIA,
            E::Amf => cfg!(windows) && self.pci_vendor == AMD,
            E::Qsv => self.pci_vendor == INTEL,
            E::Vaapi => cfg!(target_os = "linux") && self.pci_vendor != NVIDIA,
            E::VideoToolbox => cfg!(target_os = "macos"),
            E::Vulkan => self.cuda.is_some() || self.adapter.is_some() || self.node.is_some(),
        }
    }

    pub(crate) fn ran(&self, status: &crate::export::encode::EncoderStatus) -> bool {
        use vidcut_core::export::EncoderChoice as E;
        let Some(on) = status.device.as_deref() else {
            return status.kind == E::VideoToolbox;
        };
        match status.kind {
            E::Nvenc => self.cuda.is_some_and(|c| c.to_string() == on),
            E::Vulkan => self.id == on,
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
    const MICROSOFT: u32 = 0x1414;
    let cuda = crate::cuda::listed();
    // SAFETY: DXGI calls on a factory and adapters this function creates and holds.
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
            memory_mb: Some(desc.DedicatedVideoMemory as u64 >> 20).filter(|&mb| mb > 0),
            pci_vendor: desc.VendorId,
            slot,
            cuda: None,
            uuid: None,
            adapter: Some(i),
            node: None,
        });
    }
    with_cuda(&mut gpus, &cuda);
    gpus
}

#[cfg(windows)]
pub(crate) fn adapter_luid(index: u32) -> Option<[u8; 8]> {
    use ::windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    // SAFETY: DXGI's factory and the adapter at that index, as `listed`.
    let desc = unsafe {
        let factory = CreateDXGIFactory1::<IDXGIFactory1>().ok()?;
        factory.EnumAdapters1(index).ok()?.GetDesc1().ok()?
    };
    let luid = desc.AdapterLuid;
    let mut bytes = [0; 8];
    bytes[..4].copy_from_slice(&luid.LowPart.to_le_bytes());
    bytes[4..].copy_from_slice(&luid.HighPart.to_le_bytes());
    Some(bytes)
}

#[cfg(windows)]
pub(crate) fn spare(gpu: &Gpu) -> Option<u64> {
    use ::windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
        IDXGIAdapter3, IDXGIFactory1,
    };
    use ::windows::core::Interface;
    let index = gpu.adapter?;
    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    // SAFETY: DXGI's factory and the adapter at that index, queried into a struct of its own type.
    unsafe {
        let factory = CreateDXGIFactory1::<IDXGIFactory1>().ok()?;
        let adapter = factory
            .EnumAdapters1(index)
            .ok()?
            .cast::<IDXGIAdapter3>()
            .ok()?;
        adapter
            .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info)
            .ok()?;
    }
    Some(info.Budget.saturating_sub(info.CurrentUsage))
}

#[cfg(target_os = "linux")]
pub(crate) fn spare(gpu: &Gpu) -> Option<u64> {
    let slot = gpu.slot?;
    vulkan_memory()
        .into_iter()
        .find(|(at, _, _)| *at == slot)
        .and_then(|(_, _, spare)| spare)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn spare(_gpu: &Gpu) -> Option<u64> {
    None
}

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
    // SAFETY: the adapter opened by its LUID, queried into a struct of the size given, then closed.
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
    let memory = vulkan_memory();
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
        let memory_mb = slot
            .and_then(|s| memory.iter().find(|(at, _, _)| *at == s))
            .map(|(_, bytes, _)| bytes >> 20)
            .filter(|&mb| mb > 0);
        gpus.push(Gpu {
            id: id(vendor, device, slot),
            name: named,
            vendor: maker(vendor),
            memory_mb,
            pci_vendor: vendor,
            slot,
            cuda: None,
            uuid: None,
            adapter: None,
            node: Some(node),
        });
    }
    with_cuda(&mut gpus, &cuda);
    gpus
}

#[cfg(target_os = "linux")]
fn vulkan_memory() -> Vec<(Slot, u64, Option<u64>)> {
    use ash::vk;
    // SAFETY: Vulkan's loader and calls as vulkan.h declares them; the library stays loaded.
    unsafe {
        let Ok(library) = libloading::Library::new("libvulkan.so.1") else {
            return Vec::new();
        };
        let Ok(get) = library.get::<vk::PFN_vkGetInstanceProcAddr>(b"vkGetInstanceProcAddr\0")
        else {
            return Vec::new();
        };
        let entry = ash::Entry::from_static_fn(ash::StaticFn {
            get_instance_proc_addr: *get,
        });
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
        let info = vk::InstanceCreateInfo::default().application_info(&app);
        let Ok(instance) = entry.create_instance(&info, None) else {
            return Vec::new();
        };
        let mut sized = Vec::new();
        for physical in instance.enumerate_physical_devices().unwrap_or_default() {
            let extensions = instance
                .enumerate_device_extension_properties(physical)
                .unwrap_or_default();
            let has = |name: &std::ffi::CStr| {
                extensions
                    .iter()
                    .any(|e| e.extension_name_as_c_str() == Ok(name))
            };
            if !has(ash::ext::pci_bus_info::NAME) {
                continue;
            }
            let budgeted = has(ash::ext::memory_budget::NAME);
            let mut bus = vk::PhysicalDevicePCIBusInfoPropertiesEXT::default();
            let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut bus);
            instance.get_physical_device_properties2(physical, &mut props);
            if props.properties.device_type == vk::PhysicalDeviceType::INTEGRATED_GPU {
                continue;
            }
            let mut budget = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
            let mut memory = vk::PhysicalDeviceMemoryProperties2::default();
            if budgeted {
                memory = memory.push_next(&mut budget);
            }
            instance.get_physical_device_memory_properties2(physical, &mut memory);
            let largest = memory
                .memory_properties
                .memory_heaps_as_slice()
                .iter()
                .enumerate()
                .filter(|(_, h)| h.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
                .max_by_key(|(_, h)| h.size)
                .map(|(i, h)| (i, h.size));
            if let Some((heap, bytes)) = largest {
                let spare = budgeted
                    .then(|| budget.heap_budget[heap].saturating_sub(budget.heap_usage[heap]));
                sized.push((
                    (bus.pci_bus, bus.pci_device, bus.pci_function),
                    bytes,
                    spare,
                ));
            }
        }
        instance.destroy_instance(None);
        std::mem::forget(library);
        sized
    }
}

#[cfg(target_os = "linux")]
fn pci_names() -> std::collections::HashMap<(u32, u32), String> {
    let mut names = std::collections::HashMap::new();
    let Some(text) = ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
    else {
        return names;
    };
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

#[cfg(not(any(windows, target_os = "linux")))]
fn listed() -> Vec<Gpu> {
    let mut gpus = Vec::new();
    with_cuda(&mut gpus, &crate::cuda::listed());
    gpus
}

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
            g.uuid = c.uuid;
            g.name = c.name.clone();
        }
    }
    for c in cuda {
        if gpus.iter().any(|g| g.cuda == Some(c.index)) {
            continue;
        }
        let device = c.slot.and_then(pci_device).unwrap_or(0);
        gpus.push(Gpu {
            id: id(NVIDIA, device, c.slot),
            name: c.name.clone(),
            vendor: maker(NVIDIA),
            memory_mb: None,
            pci_vendor: NVIDIA,
            slot: c.slot,
            cuda: Some(c.index),
            uuid: c.uuid,
            adapter: None,
            node: None,
        });
    }
}

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
    use super::{Gpu, NVIDIA, find, id};

    fn gpu(vendor: u32, device: u32, slot: Option<(u32, u32, u32)>) -> Gpu {
        Gpu {
            id: id(vendor, device, slot),
            name: String::new(),
            vendor: String::new(),
            memory_mb: None,
            pci_vendor: vendor,
            slot,
            cuda: None,
            uuid: None,
            adapter: None,
            node: None,
        }
    }

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
        let twins = [
            gpu(NVIDIA, 0x2702, Some((1, 0, 0))),
            gpu(NVIDIA, 0x2702, Some((2, 0, 0))),
        ];
        assert_eq!(find(&twins, "10de:2702@02:00.0"), Some(&twins[1]));
        assert_eq!(find(&twins, "10de:2702@03:00.0"), None);
    }
}

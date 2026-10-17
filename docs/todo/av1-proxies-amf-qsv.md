# Hardware AV1 proxies on AMF and QSV

**When:** next time on a GPU with AV1 on AMF or QSV (AMD's RDNA 3 and later, Intel's Arc and later).

Proxies take AV1 from NVENC and VAAPI only (`ProxyEncoder`). Check AMF's and QSV's AV1 proxies against the barcode fixtures, their keyframes and frame order too (`tests/proxy.rs`), then let proxies take them.

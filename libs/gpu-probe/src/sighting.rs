//! Where a signature has been seen: a row of a registry entry, read into typed values.

use core::str::FromStr;

use platforms::OS;
use serde_json::Value;
use target_lexicon::Architecture;
use wgpu_hal::auxil::db;

/// The browsers a row can name that the `webbrowser` crate does not, each beside the name a row gives it.
const BROWSERS: [(&str, Browser); 10] = [
    ("brave", Browser::Brave),
    ("chromium", Browser::Chromium),
    ("deno", Browser::Deno),
    ("edge", Browser::Edge),
    ("gnome-web", Browser::GnomeWeb),
    ("librewolf", Browser::LibreWolf),
    ("mullvad-browser", Browser::Mullvad),
    ("samsung-internet", Browser::SamsungInternet),
    ("tor-browser", Browser::Tor),
    ("none", Browser::Absent),
];

/// The GPU vendors a row can name, each beside the name a row gives it.
const GPU_VENDORS: [(&str, GpuVendor); 13] = [
    ("amd", GpuVendor::Amd),
    ("apple", GpuVendor::Apple),
    ("arm", GpuVendor::Arm),
    ("broadcom", GpuVendor::Broadcom),
    ("google", GpuVendor::Google),
    ("imgtec", GpuVendor::ImgTec),
    ("intel", GpuVendor::Intel),
    ("mesa", GpuVendor::Mesa),
    ("microsoft", GpuVendor::Microsoft),
    ("nvidia", GpuVendor::Nvidia),
    ("qualcomm", GpuVendor::Qualcomm),
    ("unknown", GpuVendor::Unknown),
    ("none", GpuVendor::Absent),
];

/// Each GPU vendor beside the PCI vendor ID an adapter of its reports.
///
/// The IDs wgpu knows are wgpu's own. Google's and Microsoft's are those of their software renderers, `SwiftShader` and
/// the Basic Render Driver, which wgpu has no name for.
const GPU_VENDOR_IDS: [(u32, GpuVendor); 11] = [
    (db::amd::VENDOR, GpuVendor::Amd),
    (db::apple::VENDOR, GpuVendor::Apple),
    (db::arm::VENDOR, GpuVendor::Arm),
    (db::broadcom::VENDOR, GpuVendor::Broadcom),
    (0x1AE0, GpuVendor::Google),
    (db::imgtec::VENDOR, GpuVendor::ImgTec),
    (db::intel::VENDOR, GpuVendor::Intel),
    (db::mesa::VENDOR, GpuVendor::Mesa),
    (0x1414, GpuVendor::Microsoft),
    (db::nvidia::VENDOR, GpuVendor::Nvidia),
    (db::qualcomm::VENDOR, GpuVendor::Qualcomm),
];

/// The graphics backends a row can name, each beside the name a row gives it.
const BACKENDS: [(&str, Backend); 8] = [
    ("metal", Backend::Metal),
    ("d3d11", Backend::D3d11),
    ("d3d12", Backend::D3d12),
    ("vulkan", Backend::Vulkan),
    ("opengl", Backend::OpenGl),
    ("opengl-es", Backend::OpenGlEs),
    ("unknown", Backend::Unknown),
    ("none", Backend::Absent),
];

/// The windows a row can name, each beside the name a row gives it.
const WINDOWS: [(&str, Window); 4] = [
    ("normal", Window::Normal),
    ("private", Window::Private),
    ("both", Window::Both),
    ("none", Window::Absent),
];

/// Every field a row may carry: the six it must, then its free text.
const FIELDS: [&str; 18] = [
    "browser",
    "os",
    "cpu_arch",
    "gpu",
    "backend",
    "window",
    "browser_version",
    "os_version",
    "cpu_version",
    "gpu_version",
    "driver_version",
    "api_version",
    "device",
    "runtime",
    "defence",
    "reported",
    "date",
    "source",
];

/// What is wrong with a registry row that cannot be read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RowError {
    /// The row, or the entry it should be in, lacks something every one carries.
    #[error("the registry has no `{0}` where one is required")]
    Missing(&'static str),
    /// The row gives a field a value the registry does not know, or carries a field it does not know.
    #[error("the registry gives `{field}` as `{value}`, which is not one it knows")]
    Unknown {
        /// The field the value was given for.
        field: &'static str,
        /// The value the row gave.
        value: String,
    },
}

/// The browser a signature was seen in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Browser {
    /// A browser the `webbrowser` crate names, such as Chrome, Firefox or Safari.
    Listed(webbrowser::Browser),
    /// Brave.
    Brave,
    /// Chromium itself, as distinct from the browsers built on it.
    Chromium,
    /// Deno, which runs a page's modules with no window, and gives them WebGPU.
    Deno,
    /// Microsoft Edge.
    Edge,
    /// GNOME Web, once named Epiphany, whose engine is `WebKitGTK`.
    GnomeWeb,
    /// `LibreWolf`.
    LibreWolf,
    /// Mullvad Browser, Tor Browser's fingerprinting defences without the Tor network.
    Mullvad,
    /// Samsung Internet.
    SamsungInternet,
    /// Tor Browser.
    Tor,
    /// No browser: the probes ran natively.
    Absent,
}

impl FromStr for Browser {
    type Err = RowError;

    fn from_str(name: &str) -> Result<Self, RowError> {
        name.parse()
            .map(Self::Listed)
            .or_else(|_unlisted: webbrowser::ParseBrowserError| named(&BROWSERS, "browser", name))
    }
}

/// Who makes the GPU a signature was seen on, as wgpu names GPU vendors, with those it has no name for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GpuVendor {
    /// AMD.
    Amd,
    /// Apple.
    Apple,
    /// Arm, whose GPUs are the Mali line.
    Arm,
    /// Broadcom.
    Broadcom,
    /// Google, whose `SwiftShader` renders on the CPU.
    Google,
    /// Imagination Technologies, whose GPUs are the `PowerVR` line.
    ImgTec,
    /// Intel.
    Intel,
    /// Mesa, whose llvmpipe renders on the CPU.
    Mesa,
    /// Microsoft, whose Basic Render Driver renders on the CPU.
    Microsoft,
    /// NVIDIA.
    Nvidia,
    /// Qualcomm, whose GPUs are the Adreno line.
    Qualcomm,
    /// A GPU was involved, and whose it was is not on record.
    Unknown,
    /// No GPU was involved.
    Absent,
}

impl GpuVendor {
    /// The vendor whose adapters report `id` as their PCI vendor ID, or [`Self::Unknown`].
    ///
    /// wgpu gives an adapter's vendor as this ID, though not on every backend: Metal reports none.
    #[must_use]
    pub fn from_id(id: u32) -> Self {
        GPU_VENDOR_IDS
            .iter()
            .find(|vendor| vendor.0 == id)
            .map_or(Self::Unknown, |vendor| vendor.1)
    }

    /// The vendor whose name the GPU's own name opens with, or [`Self::Unknown`], for a backend that gives no ID.
    ///
    /// Metal names its GPUs this way -- "Apple M5 Pro", "AMD Radeon Pro 5500M", "Intel(R) UHD Graphics 630" -- so the
    /// name says whose GPU it is where the ID cannot, an Apple GPU and one in an Intel Mac alike.
    #[must_use]
    pub fn from_device_name(device: &str) -> Self {
        let first = device
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect::<String>()
            .to_ascii_lowercase();
        GPU_VENDORS
            .iter()
            .find(|vendor| vendor.0 == first)
            .map_or(Self::Unknown, |vendor| vendor.1)
    }

    /// The name a registry row gives the vendor, which reads back as it.
    #[must_use]
    pub fn name(self) -> &'static str {
        GPU_VENDORS
            .iter()
            .find(|vendor| vendor.1 == self)
            .map_or("unknown", |vendor| vendor.0)
    }
}

impl FromStr for GpuVendor {
    type Err = RowError;

    fn from_str(name: &str) -> Result<Self, RowError> {
        named(&GPU_VENDORS, "gpu", name)
    }
}

/// The graphics API a signature reached the GPU through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Backend {
    /// Metal.
    Metal,
    /// Direct3D 11.
    D3d11,
    /// Direct3D 12.
    D3d12,
    /// Vulkan.
    Vulkan,
    /// OpenGL.
    OpenGl,
    /// OpenGL ES.
    OpenGlEs,
    /// A graphics API was involved, and which is not on record.
    Unknown,
    /// No graphics API was involved.
    Absent,
}

impl FromStr for Backend {
    type Err = RowError;

    fn from_str(name: &str) -> Result<Self, RowError> {
        named(&BACKENDS, "backend", name)
    }
}

/// The kind of browser window a signature was seen in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Window {
    /// An ordinary window.
    Normal,
    /// A private, or incognito, window.
    Private,
    /// An ordinary window and a private one alike, each of which gave the same signature.
    Both,
    /// No browser was involved, so no window was.
    Absent,
}

impl FromStr for Window {
    type Err = RowError;

    fn from_str(name: &str) -> Result<Self, RowError> {
        named(&WINDOWS, "window", name)
    }
}

/// One place a signature has been seen: what a row of a registry entry names from fixed sets of values.
///
/// A row's free text -- its versions, the device, where the sighting is on record -- stays in the registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Sighting {
    /// The graphics API the signature reached the GPU through.
    pub backend: Backend,
    /// The browser the signature was seen in.
    pub browser: Browser,
    /// The architecture of the CPU, as `target-lexicon` names architectures.
    pub cpu_arch: Architecture,
    /// Who makes the GPU.
    pub gpu: GpuVendor,
    /// The operating system, as `std::env::consts::OS` names it.
    pub system: OS,
    /// The kind of browser window.
    pub window: Window,
}

impl Sighting {
    /// Read `row`, an object as a registry entry lists them under `seen`.
    ///
    /// A row has to give each of `browser`, `os`, `cpu_arch`, `gpu`, `backend` and `window` a value the registry knows,
    /// and may carry no field the registry does not know.
    pub fn read(row: &Value) -> Result<Self, RowError> {
        let stray = row
            .as_object()
            .into_iter()
            .flatten()
            .map(|field| field.0)
            .find(|field| !FIELDS.contains(&field.as_str()));
        if let Some(field) = stray {
            return Err(RowError::Unknown {
                field: "field",
                value: field.clone(),
            });
        }
        let cpu_arch = required(row, "cpu_arch")?;
        let system = required(row, "os")?;
        Ok(Self {
            backend: required(row, "backend")?.parse()?,
            browser: required(row, "browser")?.parse()?,
            cpu_arch: cpu_arch.parse().ok().ok_or_else(|| RowError::Unknown {
                field: "cpu_arch",
                value: cpu_arch.to_owned(),
            })?,
            gpu: required(row, "gpu")?.parse()?,
            system: system.parse().ok().ok_or_else(|| RowError::Unknown {
                field: "os",
                value: system.to_owned(),
            })?,
            window: required(row, "window")?.parse()?,
        })
    }

    /// Read every row `entry`, an entry of the registry, lists under `seen`, in the order it lists them.
    ///
    /// An entry has to carry a `seen` list, and each row in it has to read as [`Self::read`] requires.
    pub fn read_entry(entry: &Value) -> Result<Vec<Self>, RowError> {
        entry
            .get("seen")
            .and_then(Value::as_array)
            .ok_or(RowError::Missing("seen"))?
            .iter()
            .map(Self::read)
            .collect()
    }

    /// Whether this sighting was on the operating system and CPU architecture this code is running on.
    #[must_use]
    pub fn is_here(&self) -> bool {
        std::env::consts::OS
            .parse()
            .is_ok_and(|system: OS| system == self.system)
            && self.cpu_arch.to_string() == std::env::consts::ARCH
    }
}

/// The value `name` stands for in `table`, which lists each name `field` can take beside its value.
fn named<Kind>(table: &[(&str, Kind)], field: &'static str, name: &str) -> Result<Kind, RowError>
where
    Kind: Copy,
{
    table
        .iter()
        .find(|known| known.0 == name)
        .map(|known| known.1)
        .ok_or_else(|| RowError::Unknown {
            field,
            value: name.to_owned(),
        })
}

/// The text `row` gives `field`, which every row has to give.
fn required<'row>(row: &'row Value, field: &'static str) -> Result<&'row str, RowError> {
    row.get(field).and_then(Value::as_str).ok_or(RowError::Missing(field))
}

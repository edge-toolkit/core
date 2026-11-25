//! Where a signature has been seen: a row of a registry entry, read into typed values.
//!
//! Each field a row names from a fixed set also takes a value outside it, kept as its text, so a device, browser or
//! system the registry has no name for yet is recorded rather than refused.

use core::convert::Infallible;
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
const FIELDS: [&str; 20] = [
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
    "driver",
    "driver_version",
    "api_version",
    "device",
    "runtime",
    "environment",
    "defence",
    "reported",
    "date",
    "source",
];

/// What is wrong with a registry row that cannot be read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RowError {
    /// The row, or the entry it should be in, lacks something every one carries, or gives it as empty text.
    #[error("the registry has no `{0}` where one is required")]
    Missing(&'static str),
    /// The row carries a field the registry does not know.
    #[error("the registry gives `{field}` as `{value}`, which is not one it knows")]
    Unknown {
        /// The field the value was given for.
        field: &'static str,
        /// The value the row gave.
        value: String,
    },
}

/// A value another crate names, or the text a row gives where that crate has no name for it.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Named<Kind> {
    /// A value the crate names.
    Known(Kind),
    /// A value the crate has no name for, as the row gives it.
    Other(String),
}

impl<Kind> core::fmt::Display for Named<Kind>
where
    Kind: core::fmt::Display,
{
    /// The name a row gives the value: the crate's name for it, or the text the row gave.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Known(value) => value.fmt(f),
            Self::Other(text) => f.write_str(text),
        }
    }
}

/// The browser a signature was seen in.
#[derive(Clone, Debug, Eq, PartialEq)]
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
    /// A browser the registry has no name for, as the row names it.
    Other(String),
}

impl FromStr for Browser {
    type Err = Infallible;

    fn from_str(name: &str) -> Result<Self, Infallible> {
        Ok(name.parse().map_or_else(
            |_unlisted: webbrowser::ParseBrowserError| named(&BROWSERS, name, Self::Other),
            Self::Listed,
        ))
    }
}

/// Who makes the GPU a signature was seen on, as wgpu names GPU vendors, with those it has no name for.
#[derive(Clone, Debug, Eq, PartialEq)]
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
    /// A vendor the registry has no name for, as the row names it.
    Other(String),
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
            .map_or(Self::Unknown, |vendor| vendor.1.clone())
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
            .map_or(Self::Unknown, |vendor| vendor.1.clone())
    }

    /// The name a registry row gives the vendor, which reads back as it.
    #[must_use]
    pub fn name(&self) -> &str {
        if let Self::Other(name) = self {
            return name;
        }
        GPU_VENDORS
            .iter()
            .find(|vendor| vendor.1 == *self)
            .map_or("unknown", |vendor| vendor.0)
    }
}

impl FromStr for GpuVendor {
    type Err = Infallible;

    fn from_str(name: &str) -> Result<Self, Infallible> {
        Ok(named(&GPU_VENDORS, name, Self::Other))
    }
}

/// The graphics API a signature reached the GPU through.
#[derive(Clone, Debug, Eq, PartialEq)]
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
    /// A graphics API the registry has no name for, as the row names it.
    Other(String),
}

impl FromStr for Backend {
    type Err = Infallible;

    fn from_str(name: &str) -> Result<Self, Infallible> {
        Ok(named(&BACKENDS, name, Self::Other))
    }
}

/// The kind of browser window a signature was seen in.
#[derive(Clone, Debug, Eq, PartialEq)]
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
    /// A kind of window the registry has no name for, as the row names it.
    Other(String),
}

impl FromStr for Window {
    type Err = Infallible;

    fn from_str(name: &str) -> Result<Self, Infallible> {
        Ok(named(&WINDOWS, name, Self::Other))
    }
}

/// One place a signature has been seen: what a row of a registry entry names from fixed sets of values.
///
/// A row's free text -- its versions, the device, where the sighting is on record -- stays in the registry.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Sighting {
    /// The graphics API the signature reached the GPU through.
    pub backend: Backend,
    /// The browser the signature was seen in.
    pub browser: Browser,
    /// The architecture of the CPU, as `target-lexicon` names architectures.
    pub cpu_arch: Named<Architecture>,
    /// Who makes the GPU.
    pub gpu: GpuVendor,
    /// The operating system, as `std::env::consts::OS` names it.
    pub system: Named<OS>,
    /// The kind of browser window.
    pub window: Window,
}

impl Sighting {
    /// Read `row`, an object as a registry entry lists them under `seen`.
    ///
    /// A row has to give each of `browser`, `os`, `cpu_arch`, `gpu`, `backend` and `window` a value, and may carry no
    /// field the registry does not know. A value outside the set the registry names for its field is kept as its text.
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
        let Ok(backend) = required(row, "backend")?.parse();
        let Ok(browser) = required(row, "browser")?.parse();
        let Ok(gpu) = required(row, "gpu")?.parse();
        let Ok(window) = required(row, "window")?.parse();
        Ok(Self {
            backend,
            browser,
            cpu_arch: foreign(required(row, "cpu_arch")?),
            gpu,
            system: foreign(required(row, "os")?),
            window,
        })
    }

    /// Each field this sighting gives a value outside the set the registry names for it, in the order a row has them.
    #[must_use]
    pub fn others(&self) -> Vec<&'static str> {
        [
            ("browser", matches!(self.browser, Browser::Other(_))),
            ("os", matches!(self.system, Named::Other(_))),
            ("cpu_arch", matches!(self.cpu_arch, Named::Other(_))),
            ("gpu", matches!(self.gpu, GpuVendor::Other(_))),
            ("backend", matches!(self.backend, Backend::Other(_))),
            ("window", matches!(self.window, Window::Other(_))),
        ]
        .into_iter()
        .filter(|field| field.1)
        .map(|field| field.0)
        .collect()
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
        self.system.to_string() == std::env::consts::OS && self.cpu_arch.to_string() == std::env::consts::ARCH
    }
}

/// The value `name` stands for in `table`, or else `other` holding the name.
///
/// `table` lists each name a field can take beside the value it stands for.
fn named<Kind>(table: &[(&str, Kind)], name: &str, other: fn(String) -> Kind) -> Kind
where
    Kind: Clone,
{
    table
        .iter()
        .find(|known| known.0 == name)
        .map_or_else(|| other(name.to_owned()), |known| known.1.clone())
}

/// The value another crate parses `name` as, or `name` itself where that crate has no name for it.
fn foreign<Kind>(name: &str) -> Named<Kind>
where
    Kind: FromStr,
{
    name.parse()
        .map_or_else(|_unnamed| Named::Other(name.to_owned()), Named::Known)
}

/// The text `row` gives `field`, which every row has to give, and not as empty text.
fn required<'row>(row: &'row Value, field: &'static str) -> Result<&'row str, RowError> {
    row.get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or(RowError::Missing(field))
}

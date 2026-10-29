// The sidebar: every file under docs/ in reading order, grouped as
// docs/README.md groups them. A file's label defaults to its first heading;
// `label` gives a shorter one where the heading is long. nav.test.ts fails when
// a doc is missing here or an entry names a file that doesn't exist.
export interface NavEntry {
  /** The path under docs/, such as `firmware/device-api.md`. */
  file: string;
  label?: string;
}

export interface NavGroup {
  label: string;
  entries: NavEntry[];
}

export const NAV: NavGroup[] = [
  {
    label: "Start",
    entries: [
      { file: "README.md", label: "Overview" },
      { file: "roadmap.md", label: "Roadmap" },
      { file: "glossary.md", label: "Glossary" },
      { file: "evidence.md", label: "Evidence levels" },
      { file: "references.md", label: "References" },
    ],
  },
  {
    label: "Using Wellfed",
    entries: [
      { file: "devices/README.md", label: "Supported devices" },
      { file: "firmware/consumer-installation.md", label: "Installation" },
    ],
  },
  {
    label: "How it works",
    entries: [
      { file: "firmware/product-architecture.md", label: "Product architecture" },
      { file: "firmware/README.md", label: "Firmware architecture" },
      { file: "firmware/device-api.md", label: "Device API" },
      { file: "firmware/pairing.md", label: "Pairing" },
      { file: "firmware/bluetooth.md", label: "Bluetooth" },
      { file: "firmware/ota-design.md", label: "OTA design" },
      { file: "firmware/bank-policy.md", label: "Bank policy" },
      { file: "firmware/startup-diagnostics.md", label: "Startup diagnostics" },
      { file: "firmware/qa.md", label: "QA firmware" },
      { file: "firmware/resource-budget.md", label: "Resource budget" },
      { file: "firmware/device-layer.md", label: "Device layer" },
      { file: "firmware/bring-up-probes.md", label: "Bring-up probes" },
    ],
  },
  {
    label: "Feeder components",
    entries: [
      { file: "firmware/schedules.md", label: "Schedules" },
      { file: "firmware/motor-pulse.md", label: "Motor pulse" },
      { file: "firmware/calibration.md", label: "Portions and calibration" },
      { file: "firmware/jams-and-sensor-faults.md", label: "Jams and sensor faults" },
      { file: "firmware/optical-inputs.md", label: "Optical inputs" },
      { file: "firmware/buttons.md", label: "Buttons" },
      { file: "firmware/indicators.md", label: "Indicators" },
      { file: "firmware/audio.md", label: "Audio" },
      { file: "firmware/clock.md", label: "Clock" },
      { file: "firmware/power.md", label: "Power" },
      { file: "firmware/desiccant.md", label: "Desiccant reminder" },
    ],
  },
  {
    label: "Server and web",
    entries: [
      { file: "server/README.md", label: "The server" },
      { file: "server/running.md", label: "Running the server" },
      { file: "server/design.md", label: "Server design" },
      { file: "web/README.md", label: "Web UI design" },
      { file: "web/site.md", label: "well-fed.dev" },
      { file: "web/docs-site.md", label: "docs.well-fed.dev" },
    ],
  },
  {
    label: "Hardware",
    entries: [
      { file: "devices/plaf103/README.md", label: "PLAF103" },
      { file: "devices/plaf103/board.md", label: "Board and signal map" },
      { file: "devices/plaf103/motor-driver.md", label: "Motor driver" },
      { file: "devices/plaf103/uart.md", label: "UART" },
      { file: "devices/plaf103/flash-layout.md", label: "Flash layout" },
      { file: "devices/plaf103/recovery.md", label: "Recovery" },
      { file: "devices/plaf103/bench.md", label: "Relay bench" },
      { file: "devices/plaf103/stock/README.md", label: "Stock behavior" },
      { file: "devices/plaf103/stock/ota-requirements.md", label: "Stock OTA requirements" },
      { file: "chips/rtl8721dm/README.md", label: "RTL8721DM" },
      { file: "chips/rtl8721dm/pins.md", label: "RTL8721DM pins" },
      { file: "devices/_template/README.md", label: "Device template" },
    ],
  },
  {
    label: "Contributing",
    entries: [
      { file: "development/README.md", label: "Development setup" },
      { file: "development/writing.md", label: "Writing docs" },
      { file: "development/rust.md", label: "Rust standard" },
      { file: "development/typescript.md", label: "TypeScript standard" },
      { file: "development/firmware-builds.md", label: "SDK builds" },
      { file: "development/stock-server.md", label: "Stock server" },
      { file: "development/rust-flash.md", label: "Rust flash client" },
      { file: "development/uart-development.md", label: "UART development updates" },
      { file: "development/fixtures.md", label: "Optional fixtures" },
      { file: "development/provenance.md", label: "Import provenance" },
    ],
  },
];

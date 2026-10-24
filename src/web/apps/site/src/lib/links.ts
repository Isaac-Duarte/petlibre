// Every outbound link in one place, so moving the repository or a doc is one edit.
export const SITE = "https://petlibre.dev";
export const REPO = "https://github.com/Isaac-Duarte/petlibre";
export const MAINTAINER = "https://github.com/Isaac-Duarte";

const doc = (path: string) => `${REPO}/blob/master/${path}`;

export const DOCS = {
  devices: doc("docs/devices/README.md"),
  plaf103: doc("docs/devices/plaf103/README.md"),
  uart: doc("docs/devices/plaf103/uart.md"),
  recovery: doc("docs/devices/plaf103/recovery.md"),
  architecture: doc("docs/firmware/product-architecture.md"),
  development: doc("docs/development/README.md"),
  contributing: doc("CONTRIBUTING.md"),
  license: doc("LICENSE"),
  assets: doc("assets/README.md#license"),
} as const;

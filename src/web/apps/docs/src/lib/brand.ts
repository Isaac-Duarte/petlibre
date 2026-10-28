// The brand files are imported from assets/brand/, their one source, so the
// docs never carry a copy that drifts from the landing page and the app.
export { default as favicon } from "../../../../../../assets/brand/favicon.svg?url";
export { default as lockup } from "../../../../../../assets/brand/lockup.svg?url";
export { default as lockupOnLight } from "../../../../../../assets/brand/lockup-on-light.svg?url";

import tokens from "../../../../../../assets/brand/tokens.json";

/** The browser chrome color: the page's background in each theme. */
export const themeColor = { light: tokens.color.mist, dark: tokens.color.graphite };

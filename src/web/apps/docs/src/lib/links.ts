// Every outbound link in one place, so moving the repository or the site is one edit.
export const SITE = "https://docs.well-fed.dev";
export const HOME = "https://well-fed.dev";
export const REPO = "https://github.com/Isaac-Duarte/wellfed";

/** A file or folder in the repository on GitHub, from its path at the root. */
export const blob = (path: string) => `${REPO}/blob/master/${path}`;
export const edit = (path: string) => `${REPO}/edit/master/${path}`;

/** The social card well-fed.dev already serves, rendered from assets/brand/og.svg. */
export const OG_IMAGE = `${HOME}/og.png`;

// The shape of a rendered doc, shared by the build-time renderer and the pages.
export interface Heading {
  depth: 2 | 3;
  id: string;
  text: string;
}

export interface Doc {
  /** The path under docs/, such as `firmware/device-api.md`. */
  file: string;
  /** The route without its leading slash; a folder's README is the folder. */
  slug: string;
  title: string;
  description: string;
  html: string;
  headings: Heading[];
  editUrl: string;
}

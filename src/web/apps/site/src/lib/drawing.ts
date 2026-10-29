// The hero's line drawing, traced from the 3D model by assets/model/lineart.py
// and imported from assets/renders/, its one source. It is data, not an SVG
// file, so the page draws it in its own tokens without injecting markup.
import traced from "../../../../../../assets/renders/card-lines.json";

export interface Drawing {
  /** x, y, width and height of the traced lines, in the camera frame's units. */
  viewBox: number[];
  /** Visible hotspots by render.py's part names, projected into the same units. */
  anchors: Record<string, number[]>;
  /**
   * Paths in draw order. `part` is body, bowl, window, icon or food; `line` is
   * outline (a silhouette), crease or edge (an open boundary).
   */
  groups: { part: string; line: string; paths: string[] }[];
}

export const drawing: Drawing = traced;

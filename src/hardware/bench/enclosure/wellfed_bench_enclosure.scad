// Wellfed Bench — two-part printed enclosure.
//
// Board: 75 x 55 mm, r4 corners, 4x M3 holes 4 mm in from each corner.
// Coordinates below are board-local: X = KiCad X - 100, Y = 155 - KiCad Y
// (Y flipped so the J4/USB-A edge sits on Y = 0), Z = 0 at the bottom of the base.
//
// Assembly: drop the board onto the base standoffs, put the 3 button caps
// through the lid from inside, push the light pipes in, lower the lid, then
// 4x M3x12 socket-head screws from the top, through the board into M3 nuts
// pressed into hex pockets under the base (sized for SLA resin, which cracks
// if screws self-tap into it).
//
// Render: openscad -D 'part="base"' -o base.stl wellfed_bench_enclosure.scad
// part = "base" | "lid" | "button" | "preview" | "exploded"

part = "preview";

expansion_cutout = false; // true: open the lid over J6 (2x8) and J7 (SWD)

$fn = 48;

// ---- board ------------------------------------------------------------
pcb_w = 75; pcb_h = 55; pcb_r = 4; pcb_t = 1.6;
holes = [[4, 4], [71, 4], [4, 51], [71, 51]];
function P(kx, ky) = [kx - 100, 155 - ky];

// ---- shell ------------------------------------------------------------
gap    = 0.5;  // board edge to inner wall
wall   = 3.3;          // lip + clearance + lid skirt: 1.5 + 0.3 + 1.5
floor_t = 2.0;
top_t  = 2.0;
standoff_h = 4.0;              // clears THT pins on the bottom
z_pcb  = floor_t + standoff_h; // board underside
z_top_pcb = z_pcb + pcb_t;     // board top face
z_ceil = 16.0;                 // inside of the lid (USB-A ~7 mm + margin)
height = z_ceil + top_t;
z_split = z_top_pcb;           // base/lid parting line
lip_h  = 1.5;
lip_w  = 1.5;                  // JLC SLA: positioning features > 1.5 mm
lip_clr = 0.3;                 // SLA tolerance; 0.15 is enough for tuned FDM
nut_af = 5.7; nut_h = 2.6;     // M3 nut (5.5 AF x 2.4) pocket
eng = 0.8;                     // engraving depth (JLC SLA minimum)
min_feature = 0.8;             // JLC SLA: engraved detail >= 0.8 mm wide

out_off = gap + wall;
W = pcb_w + 2 * out_off;
H = pcb_h + 2 * out_off;

// ---- parts (estimated heights; KiCad 3D models aren't installed) -------
usbc_z  = z_top_pcb + 1.6;          // HRO TYPE-C-31-M-12 opening centre
usbc_cut = [12.6, 7.0];             // fits a typical cable overmold
j1 = P(104, 124);                   // host USB-C, left wall
j5 = P(171, 128);                   // aux USB-C, right wall
j4_x = 153 - 100;                   // USB-A pin centroid, front wall
j4_cut = [15.2, 7.8];
j2_c = P(164.55, 112.5);            // JST-PH vertical, lid opening (kept clear of the H2 boss)
j2_cut = [9, 11];
buttons = [P(111.5, 105.4), P(119.5, 105.4), P(127.5, 105.4)];
button_labels = ["RESET", "BOOT", "RECOVER"];
sw_top = z_top_pcb + 1.5;           // TS-1187A-B-A-B
leds = [P(137, 103.6), P(141, 103.6), P(145, 103.6), P(149, 103.6)];
led_labels = ["3V3", "STAT", "UART", "FDR"];
led_top = z_top_pcb + 0.8;
pipe_d = 1.95;                      // 1.75 mm clear filament
tube_end = led_top + 0.6;

btn_hole = 5.2; btn_stem = 4.6; btn_flange = [5.0, 8.0, 1.6]; btn_gap = 0.2;

// ---- helpers ----------------------------------------------------------
module rrect(size, r) {
    translate([r, r]) offset(r = r) square([size[0] - 2 * r, size[1] - 2 * r]);
}
module outline(off) { translate([-off, -off]) rrect([pcb_w + 2 * off, pcb_h + 2 * off], pcb_r + off); }
module slab(off, z0, z1) { translate([0, 0, z0]) linear_extrude(z1 - z0) outline(off); }
module label(t, s) {
    text(t, size = s, font = "Liberation Sans:style=Bold", halign = "center", valign = "center");
}
// rounded box cut through a wall: axis "x" (left/right walls) or "y" (front)
module port_cut(axis, at, w, h, zc, r = 1.5) {
    rot = axis == "x" ? [90, 0, 90] : [90, 0, 0];
    pos = axis == "x" ? [at[0], at[1], zc] : [at[0], at[1], zc];
    translate(pos) rotate(rot) linear_extrude(12, center = true)
        translate([-w / 2, -h / 2]) rrect([w, h], r);
}

module cutouts() {
    // host and aux USB-C
    port_cut("x", [0, j1[1]], usbc_cut[0], usbc_cut[1], usbc_z);
    port_cut("x", [pcb_w, j5[1]], usbc_cut[0], usbc_cut[1], usbc_z);
    // feeder USB-A: the receptacle passes through the wall
    port_cut("y", [j4_x, 0], j4_cut[0], j4_cut[1], z_top_pcb - 0.3 + j4_cut[1] / 2, 0.8);
    // feeder UART plug, from the top
    translate([j2_c[0], j2_c[1], z_ceil - 1]) linear_extrude(top_t + 2)
        translate(-j2_cut / 2) rrect(j2_cut, 2);
    if (expansion_cutout) {
        translate([P(108.2, 0)[0], P(0, 153.3)[1], z_ceil - 1])
            linear_extrude(top_t + 2) rrect([21.4, 6.2], 1);
        translate([P(132.8, 0)[0], P(0, 153.1)[1], z_ceil - 1])
            linear_extrude(top_t + 2) rrect([7.4, 4.5], 1);
    }
}

// ---- base -------------------------------------------------------------
module base() {
    difference() {
        union() {
            difference() {
                slab(out_off, 0, z_split);
                slab(gap, floor_t, z_split + 1);
            }
            // alignment lip, inner half of the wall
            difference() {
                slab(gap + lip_w, z_split - 0.01, z_split + lip_h);
                slab(gap, z_split - 1, z_split + lip_h + 1);
            }
            for (h = holes) translate([h[0], h[1], 0])
                cylinder(d = 10, h = z_pcb);   // merges into the wall (9 would be tangent)
        }
        for (h = holes) translate([h[0], h[1], 0]) {
            translate([0, 0, -1]) cylinder(d = 3.4, h = z_pcb + 2);
            translate([0, 0, -1]) cylinder(d = nut_af / cos(30), h = nut_h + 1, $fn = 6);
        }
        cutouts();
        // bottom edge chamfer
        difference() {
            slab(out_off + 1, -1, 0.6);
            translate([0, 0, -0.01]) hull() {
                slab(out_off - 0.6, 0, 0.01);
                slab(out_off, 0.6, 0.61);
            }
        }
        // closing the glyphs fills ridges narrower than min_feature (the
        // notches inside W and N), which SLA checks flag as thin walls
        translate([pcb_w / 2, pcb_h / 2, -0.01]) mirror([1, 0, 0])
            linear_extrude(eng + 0.01) offset(r = -min_feature / 2) offset(r = min_feature / 2)
                label("WELLFED BENCH", 6);
    }
}

// ---- lid --------------------------------------------------------------
module lid() {
    difference() {
        union() {
            difference() {
                slab(out_off, z_split, height);
                slab(gap, z_split - 1, z_ceil);
                // pocket for the base lip
                slab(gap + lip_w + lip_clr, z_split - 1, z_split + lip_h + lip_clr);
            }
            for (h = holes) translate([h[0], h[1], 0]) {
                translate([0, 0, z_top_pcb]) cylinder(d = 6, h = z_ceil - z_top_pcb + 0.01);
                translate([0, 0, 11]) cylinder(d = 8.4, h = z_ceil - 11 + 0.01);
            }
            for (l = leds) translate([l[0], l[1], tube_end]) cylinder(d = pipe_d + 2.5, h = z_ceil - tube_end + 0.01);
            // button guides
            for (b = buttons) translate([b[0], b[1], z_ceil - 1.5]) cylinder(d = btn_hole + 2.4, h = 1.51);
        }
        for (h = holes) translate([h[0], h[1], 0]) {
            translate([0, 0, z_top_pcb - 1]) cylinder(d = 3.4, h = 20);
            translate([0, 0, 12]) cylinder(d = 6.0, h = 20);   // M3 socket head is 5.5
        }
        for (l = leds) translate([l[0], l[1], tube_end - 1]) cylinder(d = pipe_d, h = 20);
        for (b = buttons) translate([b[0], b[1], z_ceil - 2]) cylinder(d = btn_hole, h = 10);
        cutouts();

        // top engravings
        translate([0, 0, height - eng]) linear_extrude(eng + 1) {
            // BOOT drops a row so the three labels don't run together
            for (i = [0 : 2]) translate([buttons[i][0] + (i == 2 ? 1 : 0), buttons[i][1] - (i == 1 ? 8 : 5.2)])
                label(button_labels[i], 1.8);
            for (i = [0 : 3]) translate([leds[i][0], leds[i][1] - 2.6]) rotate(-90)
                text(led_labels[i], size = 2, font = "Liberation Sans:style=Bold", halign = "left", valign = "center");
            translate([j2_c[0], j2_c[1] - j2_cut[1] / 2 - 3]) label("FEEDER UART", 2);
            translate([26, 22]) label("WELLFED", 7);
            translate([26, 13]) label("BENCH", 5);
        }
        // side engravings, centred on the wall above each port
        translate([-out_off - 0.01, j1[1], (usbc_z + usbc_cut[1] / 2 + height) / 2])
            rotate([90, 0, -90]) mirror([1, 0, 0]) linear_extrude(eng + 0.01) label("HOST", 2.6);
        translate([pcb_w + out_off + 0.01, j5[1], (usbc_z + usbc_cut[1] / 2 + height) / 2])
            rotate([90, 0, 90]) linear_extrude(eng + 0.01) label("AUX 5V", 2.6);
        translate([j4_x, -out_off - 0.01, (z_top_pcb + j4_cut[1] + height) / 2 - 0.4])
            rotate([90, 0, 0]) mirror([0, 0, 1]) linear_extrude(eng + 0.01) label("FEEDER 5V", 1.6);
    }
}

// ---- button cap (print 3, top down) -----------------------------------
module button_cap() {
    z0 = sw_top + btn_gap;
    translate([0, 0, z0]) cylinder(d = btn_stem, h = height + 1 - z0);
    translate([0, 0, z_ceil - btn_flange[2]])
        linear_extrude(btn_flange[2]) translate([-btn_flange[0] / 2, -btn_flange[1] / 2]) rrect(btn_flange, 2.2);
}

// ---- board mock for previews ------------------------------------------
module board_mock() {
    color("#1a1a1a") translate([0, 0, z_pcb]) linear_extrude(pcb_t) outline(0);
    color("silver") {
        translate([j1[0] - 0.35, j1[1] - 4.5, z_top_pcb]) cube([7.3, 9, 3.2]);
        translate([j5[0] - 7, j5[1] - 4.5, z_top_pcb]) cube([7.3, 9, 3.2]);
        translate([j4_x - 7.2, -3.7, z_top_pcb]) cube([14.4, 15, 7]);
    }
    color("white") translate([j2_c[0] - 2.4, j2_c[1] - 4, z_top_pcb]) cube([4.5, 8, 6]);
    for (b = buttons) color("#444") translate([b[0] - 2.55, b[1] - 2.55, z_top_pcb]) cube([5.1, 5.1, 1.5]);
    for (l = leds) color("red") translate([l[0] - 0.8, l[1] - 0.4, z_top_pcb]) cube([1.6, 0.8, 0.8]);
}

if (part == "base") base();
else if (part == "lid") translate([0, 0, height]) rotate([180, 0, 0]) lid();   // prints top-down
else if (part == "button") translate([0, 0, height + 1]) rotate([180, 0, 0]) button_cap();
else if (part == "exploded") {
    color("#d8d8d8") base();
    translate([0, 0, 1.5]) board_mock();
    translate([0, 0, 30]) color("#e8e8e8", 0.9) lid();
    for (b = buttons) translate([b[0], b[1], 18]) color("orange") button_cap();
} else {
    color("#d8d8d8") base();
    board_mock();
    color("#e8e8e8") lid();
    for (b = buttons) translate([b[0], b[1], 0]) color("orange") button_cap();
}

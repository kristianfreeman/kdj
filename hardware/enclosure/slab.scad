// kdj enclosure, slab: as small as it gets. The Pi 4 with the InnoMaker
// HiFi DAC HAT lies flat on the base; the Hosyond 5" DSI screen lies flat
// on top as the lid. The screen sets the footprint, the stack the height.
// Out of the back: USB-C power and the DAC's RCA pair; out of the side,
// optionally, the Pi's USB-A (the K3, sticks).
//
//   part = "base" | "lid" | "test" (a thin plate to check the screen) | "assembly"
//
// Units are mm. Change the named values, not the geometry.

part = "assembly";

// --- The screen (Hosyond 5" 800x480 DSI) --------------------------------
screen_w = 121.1;
screen_h = 77.9;
screen_t = 7.5;          // glass + board: MEASURE
active_w = 108.0;
active_h = 64.8;
active_dx = 0;           // lit area off centre: MEASURE
active_dy = 0;
screen_holes = [[-56, -34], [56, -34], [-56, 34], [56, 34]]; // from centre: MEASURE
screen_hole_d = 2.7;

// --- The Pi 4 and the DAC on it --------------------------------------
// In the Pi's own coordinates (its mechanical drawing): x along the long
// edge from the USB-C end, y up from the USB-C / HDMI edge, z up from the
// board's top. The Pi is turned 180 degrees in the box, so that edge faces
// the back wall and its USB-A / Ethernet end the left wall.
pi_w = 85;
pi_d = 56;
pi_t = 1.6;
pi_holes = [[3.5, 3.5], [61.5, 3.5], [3.5, 52.5], [61.5, 52.5]];
standoff_h = 4;

// Ports: [name, edge ("s" = the USB-C edge, "e" = the USB-A end), position
// along that edge (centre), width, height, height of its bottom above the
// board, sticks out past the edge by, shape ("box" | "round"), show it
// through the wall?]
usb_a = true;            // the K3 and USB sticks plug in here
pi_ports = [
    ["USB-C",   "s", 11.2, 9.0,  3.3, 0,   1.2, "box",   true],
    ["HDMI 0",  "s", 26.0, 7.5,  3.5, 0,   1.2, "box",   false],
    ["HDMI 1",  "s", 39.5, 7.5,  3.5, 0,   1.2, "box",   false],
    ["Audio",   "s", 53.5, 6.0,  6.0, 0,   2.5, "round", false],
    ["USB 3",   "e",  9.0, 13.5, 16,  0,   2.2, "box",   usb_a],
    ["USB 2",   "e", 27.0, 13.5, 16,  0,   2.2, "box",   usb_a],
    ["Ethernet","e", 45.75,16.0, 13.5,0,   2.2, "box",   false],
];

// The InnoMaker HiFi DAC HAT, on the Pi's header: a 65 x 56.5 board. Where
// its jacks sit is a guess from similar PCM5122 hats: MEASURE yours.
hat_gap = 11.5;          // Pi board top to the HAT board's bottom
hat_w = 65;
hat_d = 56.5;
// [name, position along the USB-C edge (centre), diameter, height of its
// centre above the HAT board, sticks out past the edge by, show it?]
hat_jacks = [
    ["RCA L", 18, 8.5, 6.5, 9, true],
    ["RCA R", 32, 8.5, 6.5, 9, true],
    ["3.5 mm", 48, 6.0, 4.0, 3, false],
];
jack_body = [10, 12, 12];   // each jack's housing on the board: w, d, h
stack_h = hat_gap + 1.6 + jack_body.z;   // Pi board top to the top of the jacks
clearance = 2;           // jacks to the screen's back

port_gap = 1;            // around each cutout

// --- The box ---------------------------------------------------------
wall = 2.4;
floor_t = 2;
gap = 0.4;               // around the screen in its pocket
insert_d = 4.2;          // M3 heat-set inserts in the corner posts
lip = 2.5;               // bezel over the screen's edge
vent_slots = 8;
r = 3;

$fn = 48;

in_w = max(screen_w, pi_w + 10) + 2 * gap;
in_d = max(screen_h, pi_d + 10) + 2 * gap;
out_w = in_w + 2 * wall;
out_d = in_d + 2 * wall;
board_z = floor_t + standoff_h;
base_h = board_z + 1.6 + stack_h + clearance;   // up to the screen's back
lid_h = screen_t + wall;

pi_x = wall + (in_w - pi_w) / 2;
pi_y = out_d - wall - pi_d - 1.5;               // its USB-C edge near the back wall

// A point in the Pi's coordinates, in the box's (turned 180 degrees).
function placed(x, y, z) = [pi_x + pi_w - x, pi_y + pi_d - y, z];

// The holes in the walls for every port shown: the back wall for the Pi's
// USB-C edge and the HAT's jacks, the left wall for its USB-A end.
module port_cutouts() {
    for (p = pi_ports) if (p[8]) {
        z = board_z + pi_t + p[5] - port_gap;
        h = p[4] + 2 * port_gap;
        if (p[1] == "s") translate([pi_x + pi_w - p[2] - p[3] / 2 - port_gap, out_d - wall - 1, z])
            cube([p[3] + 2 * port_gap, wall + 2, h]);
        else translate([-1, pi_y + pi_d - p[2] - p[3] / 2 - port_gap, z])
            cube([wall + 2, p[3] + 2 * port_gap, h]);
    }
    for (j = hat_jacks) if (j[5])
        translate([pi_x + pi_w - j[1], out_d - wall - 1, board_z + pi_t + hat_gap + 1.6 + j[3]])
            rotate([-90, 0, 0]) cylinder(d = j[2] + 2 * port_gap, h = wall + 2);
}

module rbox(w, d, h, rr = r) {
    hull() for (x = [rr, w - rr], y = [rr, d - rr]) translate([x, y, 0]) cylinder(r = rr, h = h);
}

module posts(h) {
    for (x = [wall + 4, out_w - wall - 4], y = [wall + 4, out_d - wall - 4])
        translate([x, y, 0]) children();
}

module base() {
    difference() {
        rbox(out_w, out_d, base_h);
        translate([wall, wall, floor_t]) rbox(in_w, in_d, base_h, r - 1);
        port_cutouts();
        // Vents: slots in the floor under the Pi, and low on the right wall.
        for (i = [0 : vent_slots - 1]) translate([pi_x + 12 + i * 8, pi_y + 12, -1]) cube([3, pi_d - 24, floor_t + 2]);
        for (i = [0 : vent_slots - 1]) translate([out_w - wall - 1, wall + 10 + i * 7, floor_t + 4]) cube([wall + 2, 3, base_h * 0.5]);
    }
    for (h = pi_holes) translate(placed(h[0], h[1], floor_t))
        difference() { cylinder(d = 6, h = standoff_h); cylinder(d = 2.5, h = standoff_h + 1); }
    posts() difference() { cylinder(d = 7, h = base_h); translate([0, 0, base_h - 8]) cylinder(d = insert_d, h = 9); }
}

// The lid: the screen sits in it face down, behind a lip, held by its own holes.
module lid() {
    difference() {
        rbox(out_w, out_d, lid_h);
        translate([out_w / 2 - screen_w / 2 - gap, out_d / 2 - screen_h / 2 - gap, wall])
            cube([screen_w + 2 * gap, screen_h + 2 * gap, screen_t + 1]);
        translate([out_w / 2 + active_dx, out_d / 2 + active_dy, -0.01]) hull() {
            cube([active_w + 2 * lip, active_h + 2 * lip, 0.02], center = true);
            translate([0, 0, wall]) cube([active_w, active_h, 0.02], center = true);
        }
        for (h = screen_holes) translate([out_w / 2 + h[0], out_d / 2 + h[1], -1]) cylinder(d = screen_hole_d, h = wall + 2);
        posts() translate([0, 0, -1]) { cylinder(d = 3.4, h = lid_h + 2); cylinder(d = 6, h = 2.5); }
    }
}

module test_plate() {
    difference() {
        rbox(out_w, out_d, 1.6);
        translate([out_w / 2 + active_dx, out_d / 2 + active_dy, 0]) cube([active_w, active_h, 5], center = true);
        for (h = screen_holes) translate([out_w / 2 + h[0], out_d / 2 + h[1], -1]) cylinder(d = screen_hole_d, h = 5);
        translate([out_w / 2 - screen_w / 2, out_d / 2 - screen_h / 2, 1.2]) difference() {
            cube([screen_w, screen_h, 1]);
            translate([0.6, 0.6, -1]) cube([screen_w - 1.2, screen_h - 1.2, 3]);
        }
    }
}

// Stand-ins for what goes inside, to see the fit (not printed): the Pi 4
// with its ports and header, the DAC HAT with its jacks.
module pi_local() {
    difference() {
        cube([pi_w, pi_d, pi_t]);
        for (h = pi_holes) translate([h[0], h[1], -1]) cylinder(d = 2.7, h = 4);
    }
    for (p = pi_ports) {
        out = p[6];
        if (p[1] == "s") translate([p[2] - p[3] / 2, -out, pi_t + p[5]])
            if (p[7] == "round") translate([p[3] / 2, 0, p[4] / 2]) rotate([-90, 0, 0]) cylinder(d = p[3], h = 12);
            else cube([p[3], (p[0] == "USB-C" ? 7.5 : 7.5) + out, p[4]]);
        else translate([pi_w - 17.5 + out, p[2] - p[3] / 2, pi_t + p[5]]) cube([17.5, p[3], p[4]]);
    }
    translate([7.1, pi_d - 3.5 - 2.5, pi_t]) cube([50.8, 5, 8.5]);            // the 40-pin header
    translate([22, 25, pi_t]) cube([15, 15, 2.4]);                              // the SoC
    // The HAT, on the header.
    translate([0, pi_d - hat_d, pi_t + hat_gap]) {
        cube([hat_w, hat_d, 1.6]);
        translate([7.1, hat_d - 6, -hat_gap + 8.5]) cube([50.8, 5, hat_gap - 8.5]);   // its socket
        translate([25, 25, 1.6]) cube([8, 8, 1.2]);                                     // the DAC chip
    }
    for (j = hat_jacks) {
        hz = pi_t + hat_gap + 1.6;
        translate([j[1] - jack_body.x / 2, pi_d - hat_d, hz]) cube([jack_body.x, jack_body.y, jack_body.z]);
        translate([j[1], pi_d - hat_d, hz + j[3]]) rotate([90, 0, 0]) cylinder(d = j[2], h = j[4]);
    }
}
module ghost_pi() {
    translate([pi_x + pi_w, pi_y + pi_d, board_z]) rotate([0, 0, 180]) pi_local();
}
module ghost_screen() {
    translate([out_w / 2 - screen_w / 2, out_d / 2 - screen_h / 2, base_h]) cube([screen_w, screen_h, screen_t]);
}

echo(str("slab: ", out_w, " x ", out_d, " x ", base_h + lid_h, " mm"));

if (part == "base") base();
else if (part == "lid") lid();
else if (part == "test") test_plate();
else if (part == "lid_placed") translate([0, out_d, base_h + lid_h]) rotate([180, 0, 0]) lid();
else if (part == "ghost_pi") ghost_pi();
else if (part == "ghost_screen") ghost_screen();
else {
    color("#2a2d33") base();
    color("#8a8f98") translate([0, out_d, base_h + lid_h]) rotate([180, 0, 0]) lid();
}

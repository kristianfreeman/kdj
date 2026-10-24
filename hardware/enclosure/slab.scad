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
pi_w = 85;               // along the back wall
pi_d = 56;
pi_holes = [[3.5, 3.5], [61.5, 3.5], [3.5, 52.5], [61.5, 52.5]];
standoff_h = 4;
stack_h = 32;            // Pi board to the top of the DAC's jacks: MEASURE
clearance = 2;           // DAC top to the screen's back

// The Pi lies with its USB-C / HDMI edge against the back wall. Cutouts in
// the back wall: [x from the Pi's left edge, z above its board, w, h].
back_ports = [
    [ 3.5, -1, 11, 6],   // USB-C power
    [30, 14, 26, 13],    // DAC: RCA pair: MEASURE where they sit
];
// The Pi's USB-A / Ethernet end faces the right wall.
usb_a = true;
usb_a_port = [2, 0, 34, 17];   // [y from the Pi's back edge, z, w, h]: both USB-A stacks

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
pi_y = out_d - wall - pi_d - 1;                 // its back edge near the back wall

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
        for (p = back_ports) translate([pi_x + p[0], out_d - wall - 1, board_z + 1.6 + p[1]]) cube([p[2], wall + 2, p[3]]);
        if (usb_a) translate([out_w - wall - 1, pi_y + pi_d - usb_a_port[0] - usb_a_port[2], board_z + 1.6 + usb_a_port[1]])
            cube([wall + 2, usb_a_port[2], usb_a_port[3]]);
        // Vents: slots in the floor under the Pi, and low on the left wall.
        for (i = [0 : vent_slots - 1]) translate([pi_x + 10 + i * 8, pi_y + 12, -1]) cube([3, pi_d - 24, floor_t + 2]);
        for (i = [0 : vent_slots - 1]) translate([-1, wall + 10 + i * 7, floor_t + 4]) cube([wall + 2, 3, base_h * 0.5]);
    }
    translate([pi_x, pi_y, floor_t]) for (h = pi_holes)
        translate([h[0], h[1], 0]) difference() { cylinder(d = 6, h = standoff_h); cylinder(d = 2.5, h = standoff_h + 1); }
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

echo(str("slab: ", out_w, " x ", out_d, " x ", base_h + lid_h, " mm"));

if (part == "base") base();
else if (part == "lid") lid();
else if (part == "test") test_plate();
else {
    color("#2a2d33") base();
    color("#8a8f98") translate([0, out_d, base_h + lid_h]) rotate([180, 0, 0]) lid();
}

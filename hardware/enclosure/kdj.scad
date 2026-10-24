// kdj enclosure: a desk wedge for one deck. A Raspberry Pi 4 with the
// InnoMaker HiFi DAC HAT on standoffs in a tray, and the Hosyond 5" DSI
// touchscreen tilted back in a front bezel. Two printed parts, joined by M3
// screws into heat-set inserts.
//
//   part = "tray" | "bezel" | "test" (a thin plate to check the screen fits)
//          | "assembly" (both, for a look)
//
// Every measurement is a named value below: measure yours and change them
// here, not in the geometry. Units are mm.

part = "assembly";

// --- The screen (Hosyond 5" 800x480 DSI) --------------------------------
screen_w = 121.1;        // module outline
screen_h = 77.9;
screen_t = 7.5;          // glass + board, without anything on the back
active_w = 108.0;        // lit area
active_h = 64.8;
active_dx = 0;           // lit area's offset from centre (measure: often uneven)
active_dy = 0;
screen_holes = [[-56, -34], [56, -34], [-56, 34], [56, 34]]; // from centre: MEASURE
screen_hole_d = 2.7;     // M2.5 clearance

// --- The Pi 4 and the DAC hat on it ----------------------------------
pi_w = 85;
pi_h = 56;
pi_holes = [[3.5, 3.5], [61.5, 3.5], [3.5, 52.5], [61.5, 52.5]]; // from the corner
stack_h = 32;            // board bottom to the top of the DAC's jacks
standoff_h = 6;          // under the Pi, for its solder joints

// The Pi lies with its long side front to back, so its USB-A and Ethernet
// end faces the back wall, and its USB-C and micro-HDMI side the right wall.
// Back wall cutouts: [x from the Pi's left edge, z above its board, w, h].
ports = [
    [ 2, 0, 15, 16],     // USB-A pair
    [20, 0, 15, 16],     // USB-A pair
    [38, 0, 17, 14],     // Ethernet
];
// Right wall cutouts: [y from the Pi's front edge, z above its board, w, h].
side_ports = [
    [ 4, 0, 11, 6],      // USB-C power (or a panel-mount extension)
    [30, 16, 30, 13],    // DAC: RCA pair: MEASURE where they sit
    [64, 16, 9, 9],      // DAC: 3.5 mm: MEASURE
];

// --- The box ---------------------------------------------------------
wall = 2.4;
tilt = 18;               // screen leans back this many degrees
margin = 6;              // bezel beyond the screen module
clearance = 4;           // between the DAC's top and the screen's back
insert_d = 4.2;          // M3 heat-set insert hole
vent_slots = 7;

$fn = 48;

face_w = screen_w + 2 * (margin + wall);
face_h = screen_h + 2 * (margin + wall);
// Low enough to look good, high enough that the Pi and DAC clear the
// screen even at their front edge, where the face is lowest.
low_h = wall + standoff_h + stack_h + clearance + wall + screen_t;
high_h = low_h + face_h * sin(tilt);
face_run = face_h * cos(tilt);  // how far back the face reaches
depth = face_run;               // the face meets the back wall: no open top
pi_x = face_w - wall - 4 - pi_h; // the Pi's corner (it lies rotated 90°)
pi_y = depth - wall - 2 - pi_w;
board_z = wall + standoff_h;

module rounded_box(size, r = 3) {
    hull() for (x = [r, size.x - r], y = [r, size.y - r])
        translate([x, y, 0]) cylinder(r = r, h = size.z);
}

// The wedge's outer shape: a box whose top is the tilted face, then flat.
module shell_solid() {
    hull() {
        cube([face_w, depth, 0.01]);
        translate([0, 0, low_h]) cube([face_w, 0.01, 0.01]);
        // The back edge, up at the face's height (the face reaches the back wall).
        translate([0, depth - 0.01, high_h]) cube([face_w, 0.01, 0.01]);
        translate([0, depth - 0.01, 0]) cube([face_w, 0.01, 0.01]);
    }
}

module tray() {
    difference() {
        shell_solid();
        // Hollow, open where the bezel goes.
        translate([wall, wall, wall]) resize([face_w - 2 * wall, depth - 2 * wall, high_h]) shell_solid();
        translate([-1, -1, low_h - 0.01]) rotate([tilt, 0, 0]) cube([face_w + 2, face_h + 2, 40]);
        // Back wall ports (USB-A, Ethernet).
        for (p = ports) translate([pi_x + p[0], depth - wall - 1, board_z + 1 + p[1]])
            cube([p[2], wall + 2, p[3]]);
        // Right wall ports (USB-C, the DAC's jacks).
        for (p = side_ports) translate([face_w - wall - 1, pi_y + p[0], board_z + 1 + p[1]])
            cube([wall + 2, p[2], p[3]]);
        // Vents: low on both sides, for air to rise past the Pi.
        for (side = [-1, face_w - wall + 1], i = [0 : vent_slots - 1])
            translate([side, depth * 0.35 + i * 7, wall + 4]) cube([wall + 2, 3, 10]);
    }
    // Pi standoffs: its holes are 58 x 49 mm, here 49 across and 58 deep.
    translate([pi_x, pi_y, wall])
        for (h = pi_holes) translate([h[1], h[0], 0])
            difference() { cylinder(d = 6, h = standoff_h); cylinder(d = 2.5, h = standoff_h + 1); }
    // Posts for the bezel's screws, inside the face's corners.
    for (x = [wall + 5, face_w - wall - 5])
        translate([x, wall + 5, 0]) difference() {
            cylinder(d = 8, h = low_h - 3);
            translate([0, 0, low_h - 11]) cylinder(d = insert_d, h = 9);
        }
}

// The front plate: holds the screen from behind, a window for its lit area.
module bezel() {
    difference() {
        rounded_box([face_w, face_h, wall + screen_t]);
        // Window for the lit area, chamfered so the edge doesn't shade it.
        translate([face_w / 2 + active_dx, face_h / 2 + active_dy, -0.01])
            hull() {
                translate([0, 0, 0]) cube([active_w + 2, active_h + 2, 0.02], center = true);
                translate([0, 0, wall]) cube([active_w, active_h, 0.02], center = true);
            }
        // Pocket the screen module sits in.
        translate([(face_w - screen_w) / 2 - 0.3, (face_h - screen_h) / 2 - 0.3, wall])
            cube([screen_w + 0.6, screen_h + 0.6, screen_t + 1]);
        // Screw holes into the screen's own mounting holes.
        for (h = screen_holes) translate([face_w / 2 + h[0], face_h / 2 + h[1], -1])
            cylinder(d = screen_hole_d, h = wall + 2);
        // Screws into the tray's posts.
        for (x = [wall + 5, face_w - wall - 5]) translate([x, wall + 5, -1]) cylinder(d = 3.4, h = 20);
    }
}

// A quick fit check: just the window and holes, a few minutes to print.
module test_plate() {
    difference() {
        rounded_box([face_w, face_h, 1.6]);
        translate([face_w / 2 + active_dx, face_h / 2 + active_dy, 0])
            cube([active_w, active_h, 5], center = true);
        for (h = screen_holes) translate([face_w / 2 + h[0], face_h / 2 + h[1], -1])
            cylinder(d = screen_hole_d, h = 5);
        // Outline of the module, scored, to check against the real one.
        translate([(face_w - screen_w) / 2, (face_h - screen_h) / 2, 1.2])
            difference() {
                cube([screen_w, screen_h, 1]);
                translate([0.6, 0.6, -1]) cube([screen_w - 1.2, screen_h - 1.2, 3]);
            }
    }
}

if (part == "tray") tray();
else if (part == "bezel") bezel();
else if (part == "test") test_plate();
else {
    color("#2a2d33") tray();
    color("#8a8f98") translate([0, 0, low_h]) rotate([tilt, 0, 0]) translate([0, 0, -wall - screen_t]) bezel();
}

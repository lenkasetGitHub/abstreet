use crate::colors::ColorScheme;
use crate::objects::{Ctx, ID};
use crate::render::{
    RenderOptions, Renderable, BIG_ARROW_THICKNESS, MIN_ZOOM_FOR_MARKINGS,
    PARCEL_BOUNDARY_THICKNESS,
};
use dimensioned::si;
use ezgui::{Color, GfxCtx};
use geom::{Bounds, Circle, Line, Polygon, Pt2D};
use map_model::{
    IntersectionType, Lane, LaneID, LaneType, Map, Road, Turn, LANE_THICKNESS, PARKING_SPOT_LENGTH,
};

// Just a function to draw something later.
type Marking = Box<Fn(&mut GfxCtx, &ColorScheme)>;

pub struct DrawLane {
    pub id: LaneID,
    pub polygon: Polygon,
    markings: Vec<Marking>,
    zorder: isize,
}

impl DrawLane {
    pub fn new(lane: &Lane, map: &Map) -> DrawLane {
        let road = map.get_r(lane.parent);
        let polygon = lane.lane_center_pts.make_polygons(LANE_THICKNESS);

        let mut markings: Vec<Marking> = Vec::new();
        if road.is_canonical_lane(lane.id) {
            let lines = road.center_pts.lines();
            markings.push(Box::new(move |g, cs| {
                for line in &lines {
                    g.draw_rounded_line(
                        cs.get_def("road center line", Color::YELLOW),
                        BIG_ARROW_THICKNESS,
                        line,
                    );
                }
            }));
        }
        match lane.lane_type {
            LaneType::Sidewalk => {
                markings.push(calculate_sidewalk_lines(lane));
            }
            LaneType::Parking => {
                markings.push(calculate_parking_lines(lane));
            }
            LaneType::Driving | LaneType::Bus => {
                if let Some(m) = calculate_driving_lines(lane, road) {
                    markings.push(m);
                }
                for m in calculate_turn_markings(map, lane) {
                    markings.push(m);
                }
            }
            LaneType::Biking => {}
        };
        if lane.is_driving()
            && map.get_i(lane.dst_i).intersection_type == IntersectionType::StopSign
        {
            if let Some(m) = calculate_stop_sign_line(road, lane, map) {
                markings.push(m);
            }
        }

        DrawLane {
            id: lane.id,
            polygon,
            markings,
            zorder: road.get_zorder(),
        }
    }

    fn draw_debug(&self, g: &mut GfxCtx, ctx: &Ctx) {
        let circle_color = ctx
            .cs
            .get_def("debug line endpoint", Color::rgb_f(0.8, 0.1, 0.1));

        for l in ctx.map.get_l(self.id).lane_center_pts.lines() {
            g.draw_line(
                ctx.cs.get_def("debug line", Color::RED),
                PARCEL_BOUNDARY_THICKNESS / 2.0,
                &l,
            );
            g.draw_circle(circle_color, &Circle::new(l.pt1(), 0.4));
            g.draw_circle(circle_color, &Circle::new(l.pt2(), 0.8));
        }
    }
}

impl Renderable for DrawLane {
    fn get_id(&self) -> ID {
        ID::Lane(self.id)
    }

    fn draw(&self, g: &mut GfxCtx, opts: RenderOptions, ctx: &Ctx) {
        let color = opts.color.unwrap_or_else(|| {
            let l = ctx.map.get_l(self.id);
            match l.lane_type {
                LaneType::Driving => ctx.cs.get_def("driving lane", Color::BLACK),
                LaneType::Bus => ctx.cs.get_def("bus lane", Color::rgb(190, 74, 76)),
                LaneType::Parking => ctx.cs.get_def("parking lane", Color::grey(0.2)),
                LaneType::Sidewalk => ctx.cs.get_def("sidewalk", Color::grey(0.8)),
                LaneType::Biking => ctx.cs.get_def("bike lane", Color::rgb(15, 125, 75)),
            }
        });
        g.draw_polygon(color, &self.polygon);

        if ctx.canvas.cam_zoom >= MIN_ZOOM_FOR_MARKINGS || opts.show_all_detail {
            for m in &self.markings {
                m(g, ctx.cs);
            }
        }

        if opts.debug_mode {
            self.draw_debug(g, ctx);
        }
    }

    fn get_bounds(&self) -> Bounds {
        self.polygon.get_bounds()
    }

    fn contains_pt(&self, pt: Pt2D) -> bool {
        self.polygon.contains_pt(pt)
    }

    fn get_zorder(&self) -> isize {
        self.zorder
    }
}

// TODO this always does it at pt1
fn perp_line(l: Line, length: f64) -> Line {
    let pt1 = l.shift_right(length / 2.0).pt1();
    let pt2 = l.shift_left(length / 2.0).pt1();
    Line::new(pt1, pt2)
}

fn calculate_sidewalk_lines(lane: &Lane) -> Marking {
    let tile_every = LANE_THICKNESS * si::M;

    let length = lane.length();

    let mut lines = Vec::new();
    // Start away from the intersections
    let mut dist_along = tile_every;
    while dist_along < length - tile_every {
        let (pt, angle) = lane.dist_along(dist_along);
        // Reuse perp_line. Project away an arbitrary amount
        let pt2 = pt.project_away(1.0, angle);
        lines.push(perp_line(Line::new(pt, pt2), LANE_THICKNESS));
        dist_along += tile_every;
    }

    Box::new(move |g, cs| {
        for line in &lines {
            g.draw_line(cs.get_def("sidewalk lines", Color::grey(0.7)), 0.25, line);
        }
    })
}

fn calculate_parking_lines(lane: &Lane) -> Marking {
    // meters, but the dims get annoying below to remove
    // TODO make Pt2D natively understand meters, projecting away by an angle
    let leg_length = 1.0;

    let mut lines = Vec::new();
    let num_spots = lane.number_parking_spots();
    if num_spots > 0 {
        for idx in 0..=num_spots {
            let (pt, lane_angle) = lane.dist_along(PARKING_SPOT_LENGTH * (1.0 + idx as f64));
            let perp_angle = lane_angle.rotate_degs(270.0);
            // Find the outside of the lane. Actually, shift inside a little bit, since the line will
            // have thickness, but shouldn't really intersect the adjacent line when drawn.
            let t_pt = pt.project_away(LANE_THICKNESS * 0.4, perp_angle);
            // The perp leg
            let p1 = t_pt.project_away(leg_length, perp_angle.opposite());
            lines.push(Line::new(t_pt, p1));
            // Upper leg
            let p2 = t_pt.project_away(leg_length, lane_angle);
            lines.push(Line::new(t_pt, p2));
            // Lower leg
            let p3 = t_pt.project_away(leg_length, lane_angle.opposite());
            lines.push(Line::new(t_pt, p3));
        }
    }

    Box::new(move |g, cs| {
        for line in &lines {
            g.draw_line(cs.get_def("parking line", Color::WHITE), 0.25, line);
        }
    })
}

fn calculate_driving_lines(lane: &Lane, parent: &Road) -> Option<Marking> {
    // The leftmost lanes don't have dashed white lines.
    if parent.dir_and_offset(lane.id).1 == 0 {
        return None;
    }

    let dash_separation = 1.5 * si::M;
    let dash_len = 1.0 * si::M;

    let lane_edge_pts = lane.lane_center_pts.shift_left(LANE_THICKNESS / 2.0);
    if lane_edge_pts.length() < 2.0 * dash_separation {
        return None;
    }
    // Don't draw the dashes too close to the ends.
    let polygons = lane_edge_pts
        .slice(dash_separation, lane_edge_pts.length() - dash_separation)
        .0
        .dashed_polygons(0.25, dash_len, dash_separation);

    Some(Box::new(move |g, cs| {
        for p in &polygons {
            g.draw_polygon(cs.get_def("dashed lane line", Color::WHITE), p);
        }
    }))
}

fn calculate_stop_sign_line(road: &Road, lane: &Lane, map: &Map) -> Option<Marking> {
    if map.get_stop_sign(lane.dst_i).is_priority_lane(lane.id) {
        return None;
    }

    // TODO maybe draw the stop sign octagon on each lane?

    let (pt1, angle) = lane.safe_dist_along(lane.length() - (1.0 * si::M))?;
    // Reuse perp_line. Project away an arbitrary amount
    let pt2 = pt1.project_away(1.0, angle);
    // Don't clobber the yellow line.
    let line = if road.is_canonical_lane(lane.id) {
        perp_line(
            Line::new(pt1, pt2).shift_right(BIG_ARROW_THICKNESS / 2.0),
            LANE_THICKNESS - BIG_ARROW_THICKNESS,
        )
    } else {
        perp_line(Line::new(pt1, pt2), LANE_THICKNESS)
    };

    Some(Box::new(move |g, cs| {
        g.draw_line(cs.get_def("stop line for lane", Color::RED), 0.45, &line);
    }))
}

fn calculate_turn_markings(map: &Map, lane: &Lane) -> Vec<Marking> {
    let mut results: Vec<Marking> = Vec::new();

    // Are there multiple driving lanes on this side of the road?
    if map
        .find_closest_lane(lane.id, vec![LaneType::Driving])
        .is_err()
    {
        return results;
    }

    for turn in map.get_turns_from_lane(lane.id) {
        if let Some(m) = turn_markings(turn, map) {
            results.push(m);
        }
    }
    results
}

fn turn_markings(turn: &Turn, map: &Map) -> Option<Marking> {
    let lane = map.get_l(turn.id.src);
    let len = lane.length();
    if len < 7.0 * si::M {
        return None;
    }

    let common_base = lane
        .lane_center_pts
        .slice(len - 7.0 * si::M, len - 5.0 * si::M)
        .0;
    let base_polygon = common_base.make_polygons(0.1);
    let turn_line = Line::new(
        common_base.last_pt(),
        common_base
            .last_pt()
            .project_away(LANE_THICKNESS / 2.0, turn.angle()),
    );

    Some(Box::new(move |g, cs| {
        let color = cs.get_def("turn restrictions on lane", Color::WHITE);
        g.draw_polygon(color, &base_polygon);
        g.draw_arrow(color, 0.05, &turn_line);
    }))
}

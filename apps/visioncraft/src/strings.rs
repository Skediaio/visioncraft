//! Every piece of UI text, in one place so it can be translated later.

pub const APP_NAME: &str = "VisionCraft";

pub const OPEN: &str = "Open";
pub const SAVE: &str = "Save";
pub const SAVED: &str = "Saved";
pub const UNDO: &str = "Undo";
pub const REDO: &str = "Redo";
pub const LAYERS: &str = "Layers";
pub const CLAY: &str = "Clay";
pub const QUALITY: &str = "Quality";
pub const WALK: &str = "Walk";
pub const FPS: &str = "fps";
pub const HIDE_FPS: &str = "Hide fps";
pub const SHOW_FPS: &str = "Show fps";
pub const LOADING: &str = "Loading…";
pub const MODELS: &str = "Models";
pub const RELOAD_MODEL: &str = "Reload model";
pub const RELINK: &str = "Relink model";
pub const MODEL_UPDATED: &str = "Model updated";
pub const DROP_HINT: &str = "Open or drop a .3dcraft or .obj model, or draw with the Draw tool";
pub const WALK_HINT: &str = "Walking: WASD or arrows move, Shift runs, drag to look, Space to exit";

pub const MODE_LABELS: [(&str, &str); 5] = [
    ("Draw", "Draw"),
    ("Materials", "Mat."),
    ("Objects", "Obj."),
    ("Landscape & Sun", "Sun"),
    ("Viewpoints", "Views"),
];

// Draw
pub const TOOL_SELECT: &str = "Select";
pub const TOOL_AREA: &str = "Area";
pub const TOOL_BLOCK: &str = "Block";
pub const TOOL_WALL: &str = "Wall";
pub const DRAW_HINT: &str = "Click to add points. Click the first point or press Enter to close, Esc cancels, Backspace undoes a point. Alt turns the grid snap off.";
pub const WALL_HINT: &str = "Click to add points. Enter finishes, Esc cancels, Backspace undoes a point. Alt turns the grid snap off.";
pub const SELECT_HINT: &str = "Click a shape to select it. Drag a corner to move it, click an edge to add a corner, Delete removes the selected corner.";
pub const NAME: &str = "Name";
pub const HEIGHT: &str = "Height";
pub const BASE: &str = "Base";
pub const THICKNESS: &str = "Thickness";
pub const ELEVATION: &str = "Elevation";
pub const AREA: &str = "Area";
pub const PERIMETER: &str = "Perimeter";
pub const DELETE_SHAPE: &str = "Delete shape";
pub const DELETE_CORNER: &str = "Delete corner";

// Materials
pub const THIS_FACE: &str = "This face";
pub const WHOLE_BODY: &str = "Whole body";
pub const WHOLE_OBJECT: &str = "Whole object";
pub const EYEDROPPER: &str = "Eyedropper";
pub const MAP_SCALE: &str = "Map scale";
pub const RELIEF: &str = "Relief";
pub const GLOSS: &str = "Gloss";
pub const WEATHERING: &str = "Weathering";
pub const MATERIALS_HINT: &str =
    "Hover a surface and click it to choose a material. Drop a .mcmat file to add one.";
pub const MATERIAL_ADDED: &str = "Material added";

// Objects
pub const TOOL_SINGLE: &str = "Single";
pub const TOOL_SCATTER: &str = "Area scatter";
pub const DENSITY: &str = "Density per m²";
pub const SINGLE_HINT: &str = "Click the ground to place one.";
pub const SCATTER_HINT: &str = "Click points around the area, then Enter or click the first point.";
pub const PLANTINGS: &str = "Placed";
pub const REMOVE: &str = "Remove";

// Sun
pub const DATE: &str = "Date";
pub const TIME: &str = "Time";
pub const LATITUDE: &str = "Latitude";
pub const LONGITUDE: &str = "Longitude";
pub const UTC_OFFSET: &str = "UTC offset";
pub const CITY: &str = "City";
pub const EXPOSURE: &str = "Exposure";
pub const SUN_HINT: &str =
    "Drag the sun around the compass for the time of day, click the month dial for the season.";
pub const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
pub const CITIES: [(&str, f64, f64, f64); 6] = [
    ("Brussels", 50.85, 4.35, 1.0),
    ("Antwerp", 51.22, 4.40, 1.0),
    ("Ghent", 51.05, 3.72, 1.0),
    ("Amsterdam", 52.37, 4.90, 1.0),
    ("Paris", 48.86, 2.35, 1.0),
    ("London", 51.51, -0.13, 0.0),
];

// Viewpoints
pub const SAVE_VIEW: &str = "Save current view";
pub const UPDATE_VIEW: &str = "Update from current view";
pub const RENAME: &str = "Rename";
pub const DELETE: &str = "Delete";
pub const VIEWS_FULL: &str = "Eight viewpoints are saved. Delete one to add another.";

pub const UNSAVED_PROJECT: &str = "Untitled";
pub const RELINK_PROMPT: &str = "needs its file";

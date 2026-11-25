use crate::error::{Error, Result};
use crate::key_sets::same;
use crate::shortcuts::{Action, Bindings, Combo, Custom, Held, key_named};

const BASE: &str = "DaVinci Resolve";

const OWN: &str = "#vidcut ";

const COMMANDS: &[(&str, Action)] = &[
    ("controlPlayToggle", Action::PlayOrStop),
    ("controlPlayReverse", Action::PlayBackward),
    ("controlStop", Action::Stop),
    ("controlPlayForward", Action::PlayForward),
    ("controlStepReverse", Action::FrameBack),
    ("controlStepForward", Action::FrameForward),
    ("controlLargeStepReverse", Action::SecondBack),
    ("controlLargeStepForward", Action::SecondForward),
    ("controlClipPrev", Action::PreviousCut),
    ("controlClipNext", Action::NextCut),
    ("controlMarkersPrev", Action::PreviousMarker),
    ("controlMarkersNext", Action::NextMarker),
    ("controlTimelineStart", Action::TimelineStart),
    ("controlTimelineEnd", Action::TimelineEnd),
    ("controlTimecode", Action::GoToTimecode),
    ("controlTimecodeIncrement", Action::JumpForward),
    ("controlTimecodeDecrement", Action::JumpBack),
    ("viewViewerToggle", Action::TimelineOrSource),
    ("editScrubAudio", Action::AudioScrubbing),
    ("controlLoop", Action::Loop),
    ("workspaceViewerModeFullViewer", Action::FullScreen),
    ("workspaceViewerModeEnhancedViewer", Action::EnhancedViewer),
    ("markIn", Action::MarkIn),
    ("markOut", Action::MarkOut),
    ("controlGotoIn", Action::GoToIn),
    ("controlGotoOut", Action::GoToOut),
    ("markResetInOut", Action::ClearMarks),
    ("markMarkerAdd", Action::AddMarker),
    ("markMarkerClear", Action::DeleteMarker),
    ("markCreateSubClip", Action::CreateSubclip),
    ("editInsertOverwriteActionInsert", Action::SmartInsert),
    ("editInsertOverwriteActionOverwrite", Action::Overwrite),
    (
        "editInsertOverwriteActionRippleOverwrite",
        Action::RippleOverwrite,
    ),
    ("editInsertOverwriteActionPlaceOnTop", Action::PlaceOnTop),
    ("editInsertOverwriteActionAppendAtEnd", Action::Append),
    ("editBladeRazor", Action::Split),
    ("editM2SplitClip", Action::Split),
    ("editAddTransition", Action::Dissolve),
    ("editDelete", Action::Delete),
    ("editBackspace", Action::Delete),
    ("editCopy", Action::Copy),
    ("editCut", Action::Cut),
    ("editPaste", Action::Paste),
    ("editPasteAttributes", Action::PasteAttributes),
    ("editClipEnabled", Action::PictureOff),
    ("clipChangeClipDuration", Action::ChangeDuration),
    ("clipChangeClipSpeed", Action::ChangeSpeed),
    ("controlKeyframeNext", Action::NextKeyframe),
    ("controlKeyframePrev", Action::PreviousKeyframe),
    ("markKeyframeMakeDynamic", Action::AddKeyframe),
    ("markKeyframeDelete", Action::DeleteKeyframe),
    ("editSelectAll", Action::SelectAll),
    ("editDeselectAll", Action::SelectNothing),
    ("trimSelectAllItemsAtPlayhead", Action::SelectAtPlayhead),
    ("editSelectItemAtPlayhead", Action::SelectClipAtPlayhead),
    ("editSelectEditPoint", Action::NearestCut),
    ("editToggleEditPointType", Action::CutSide),
    ("editNudgeTrimStepNudgeReverse", Action::TrimLeft),
    ("editNudgeTrimStepNudgeForward", Action::TrimRight),
    (
        "editNudgeTrimStepTrimMultiFrameLeft",
        Action::TrimSecondLeft,
    ),
    (
        "editNudgeTrimStepTrimMultiFrameRight",
        Action::TrimSecondRight,
    ),
    ("editToggleDynamicTrimMode", Action::DynamicTrim),
    ("editSnapping", Action::Snapping),
    ("editUndo", Action::Undo),
    ("editRedo", Action::Redo),
    ("fileImportMedia", Action::Import),
    ("nodesAddSerial", Action::AddSerialNode),
    ("nodesAddBefore", Action::AddNodeBefore),
    ("nodesAddParallel", Action::AddParallelNode),
    ("nodesAddLayer", Action::AddLayerNode),
    ("nodesAppend", Action::AppendNode),
    ("nodesAddOutside", Action::AddOutsideNode),
    ("nodesAddSplitterCombiner", Action::AddSplitterCombiner),
    ("nodesAddCPW", Action::AddCircleNode),
    ("nodesAddLPW", Action::AddLinearNode),
    ("nodesAddPPW", Action::AddPolygonNode),
    ("nodesAddPCW", Action::AddCurveNode),
    ("nodesExtractCurrent", Action::ExtractNode),
    ("nodesToggleCurrent", Action::SwitchSelectedNodes),
    ("nodesToggleAll", Action::SwitchAllNodes),
    ("nodesPrevious", Action::PreviousNode),
    ("nodesNext", Action::NextNode),
    ("sessionBaseMemory", Action::ResetNodeGrade),
    ("sessionBaseMemoryAll", Action::ResetGradesKeepNodes),
    ("sessionBaseMemoryReset", Action::ResetAllGrades),
    ("markKeyframeMakeStatic", Action::AddStaticKeyframe),
    ("sessionAutoColor", Action::AutoColor),
    ("fileNewFolder", Action::NewBin),
    ("fileNewTimeline", Action::NewTimeline),
    ("resolvePreferences", Action::Preferences),
];

const SHARED: &[(&str, &[&str])] = &[
    ("editBladeRazor", &["Ctrl+B"]),
    ("editM2SplitClip", &["Ctrl+\\"]),
    ("editDelete", &["Shift+Backspace", "Delete"]),
    ("editBackspace", &["Backspace"]),
];

const NAMES: &[(&str, &str)] = &[
    ("Delete", "Del"),
    ("PageUp", "PgUp"),
    ("PageDown", "PgDown"),
    ("Enter", "Return"),
    ("Escape", "Esc"),
    ("Insert", "Ins"),
];

const STANDARD: &[(&str, &str)] = &[("Copy", "Ctrl+C"), ("Cut", "Ctrl+X"), ("Paste", "Ctrl+V")];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Read {
    pub keys: Custom,
    pub unknown: Vec<String>,
    pub unusable: Vec<String>,
}

fn ids(action: Action) -> impl Iterator<Item = &'static str> {
    COMMANDS
        .iter()
        .filter(move |(_, a)| *a == action)
        .map(|(id, _)| *id)
}

fn mapped(action: Action) -> bool {
    ids(action).next().is_some()
}

fn shared(id: &str, key: Combo) -> bool {
    SHARED
        .iter()
        .any(|(shared, own)| *shared == id && own.iter().any(|k| k.parse() == Ok(key)))
}

fn spread(action: Action, keys: &[Combo]) -> Vec<(&'static str, Vec<Combo>)> {
    let mut out: Vec<(&str, Vec<Combo>)> = ids(action).map(|id| (id, Vec::new())).collect();
    for &key in keys {
        let at = out.iter().position(|(id, _)| shared(id, key)).unwrap_or(0);
        out[at].1.push(key);
    }
    out
}

fn written(combo: &Combo) -> String {
    let key = NAMES
        .iter()
        .find(|(ours, _)| *ours == combo.key)
        .map_or(combo.key, |(_, theirs)| theirs);
    let h = combo.held;
    [
        (h.ctrl, "Ctrl"),
        (h.control, "Meta"),
        (h.alt, "Alt"),
        (h.shift, "Shift"),
        (true, key),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, name)| name)
    .collect::<Vec<_>>()
    .join("+")
}

fn line(id: &str, keys: &[Combo]) -> String {
    let keys: Vec<String> = keys.iter().map(written).collect();
    match keys.is_empty() {
        true => format!("{id} :="),
        false => format!("{id} := {}", keys.join(" | ")),
    }
}

pub fn text(custom: &Custom) -> Result<String> {
    let changes = Bindings::of(custom)?.changes();
    let theirs: Vec<String> = changes
        .iter()
        .filter(|(a, _)| mapped(**a))
        .flat_map(|(a, keys)| spread(*a, keys))
        .map(|(id, keys)| line(id, &keys))
        .collect();
    let ours: Vec<String> = changes
        .iter()
        .filter(|(a, _)| !mapped(**a))
        .map(|(a, keys)| format!("{OWN}{}", line(a.name(), keys)))
        .collect();
    let mut out = format!("import \"{BASE}\"\n");
    for lines in [theirs, ours] {
        if !lines.is_empty() {
            out.push('\n');
            for l in lines {
                out.push_str(&l);
                out.push('\n');
            }
        }
    }
    Ok(out)
}

fn combo(text: &str) -> Option<Combo> {
    let text = STANDARD
        .iter()
        .find(|(name, _)| *name == text)
        .map_or(text, |(_, combo)| combo);
    let (held, key) = match text.strip_suffix("++") {
        Some(held) => (held, "+"),
        None => match text.rsplit_once('+') {
            Some((held, key)) if !key.is_empty() => (held, key),
            _ => ("", text),
        },
    };
    let mut on = Held::default();
    for name in held.split('+').filter(|n| !n.is_empty()) {
        match name {
            "Ctrl" => on.ctrl = true,
            "Meta" if cfg!(target_os = "macos") => on.control = true,
            "Alt" => on.alt = true,
            "Shift" => on.shift = true,
            _ => return None,
        }
    }
    if key == "Enter" {
        return None;
    }
    let key = NAMES
        .iter()
        .find(|(_, theirs)| *theirs == key)
        .map_or(key, |(ours, _)| ours);
    key_named(key).map(|key| Combo { held: on, key })
}

enum Line<'a> {
    Import(&'a str),
    Command(&'a str, &'a str),
    Own(&'a str, &'a str),
}

fn parsed(text: &str) -> std::result::Result<Option<Line<'_>>, String> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix(OWN) {
        let (name, keys) = rest
            .split_once(":=")
            .ok_or_else(|| format!("{text:?} is not `{OWN}action := keys`"))?;
        return Ok(Some(Line::Own(name.trim(), keys)));
    }
    if text.is_empty() || text.starts_with('#') {
        return Ok(None);
    }
    if let Some((id, keys)) = text.split_once(":=") {
        return Ok(Some(Line::Command(id.trim(), keys)));
    }
    text.strip_prefix("import")
        .map(str::trim)
        .and_then(|t| t.strip_prefix('"'))
        .and_then(|t| t.strip_suffix('"'))
        .map(|name| Some(Line::Import(name)))
        .ok_or_else(|| format!("{text:?} is not `command := keys` or `import \"set\"`"))
}

fn listed(keys: &str) -> impl Iterator<Item = &str> {
    keys.split('|').map(str::trim).filter(|k| !k.is_empty())
}

fn combos(id: &str, keys: &str, unusable: &mut Vec<String>) -> Vec<Combo> {
    let mut combos = Vec::new();
    for key in listed(keys) {
        match combo(key) {
            Some(c) if !combos.contains(&c) => combos.push(c),
            Some(_) => {}
            None => unusable.push(format!("{id} := {key}")),
        }
    }
    combos
}

pub fn parse(text: &str, base: impl Fn(&str) -> Result<Custom>) -> Result<Read> {
    let mut set = None;
    let mut commands: Vec<(&str, Vec<Combo>)> = Vec::new();
    let mut own = Custom::new();
    let mut read = Read::default();
    for (n, text) in text.lines().enumerate() {
        let at = |why: String| Error::invalid(format!("line {}: {why}", n + 1));
        match parsed(text).map_err(at)? {
            None => {}
            Some(Line::Import(name)) => {
                if set.replace(name).is_some() {
                    return Err(at("a file has one import line".into()));
                }
            }
            Some(Line::Command(id, keys)) => {
                if !COMMANDS.iter().any(|(known, _)| *known == id) {
                    if listed(keys).next().is_some() {
                        read.unknown.push(id.to_string());
                    }
                    continue;
                }
                let combos = combos(id, keys, &mut read.unusable);
                commands.retain(|(known, _)| *known != id);
                commands.push((id, combos));
            }
            Some(Line::Own(name, keys)) => {
                let action = Action::named(name)
                    .filter(|a| !mapped(*a))
                    .ok_or_else(|| at(format!("{name:?} is not an action without a command")))?;
                let combos = combos(&format!("{OWN}{name}"), keys, &mut read.unusable);
                own.insert(action, combos);
            }
        }
    }
    let mut custom = match set {
        Some(name) if !same(name, BASE) => base(name)?,
        Some(_) => Custom::new(),
        None => COMMANDS.iter().map(|&(_, a)| (a, Vec::new())).collect(),
    };
    let start = Bindings::of(&custom)?;
    for &action in Action::ALL {
        if !ids(action).any(|id| commands.iter().any(|(c, _)| *c == id)) {
            continue;
        }
        let mut keys = Vec::new();
        for (id, was) in spread(action, start.keys(action)) {
            let now = commands
                .iter()
                .find(|(c, _)| *c == id)
                .map_or(was, |(_, k)| k.clone());
            for key in now {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        custom.insert(action, keys);
    }
    custom.extend(own);
    custom.retain(|action, keys| {
        let table = action.default_keys();
        keys.len() != table.len() || keys.iter().any(|k| !table.contains(k))
    });
    Bindings::of(&custom)?;
    read.keys = custom;
    Ok(read)
}

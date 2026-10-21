use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Play,
    Mark,
    Edit,
    Clips,
    Trim,
    App,
}

impl Group {
    pub const ALL: [Group; 6] = [
        Group::Play,
        Group::Mark,
        Group::Edit,
        Group::Clips,
        Group::Trim,
        Group::App,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Group::Play => "Play and move",
            Group::Mark => "Mark",
            Group::Edit => "Put clips in",
            Group::Clips => "Change clips",
            Group::Trim => "Select and trim",
            Group::App => "Project and app",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Shortcut {
    pub group: Group,
    pub keys: &'static [&'static str],
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<&'static str>,
}

pub fn key_name(key: &str, mac: bool) -> &str {
    match (key, mac) {
        ("Ctrl", true) => "Cmd",
        ("Alt", true) => "Option",
        _ => key,
    }
}

const fn key(
    group: Group,
    keys: &'static [&'static str],
    action: &'static str,
    explanation: Option<&'static str>,
) -> Shortcut {
    Shortcut {
        group,
        keys,
        action,
        explanation,
    }
}

use Group::{App, Clips, Edit, Mark, Play, Trim};

pub const SHORTCUTS: &[Shortcut] = &[
    key(
        Play,
        &["Space"],
        "Play or stop",
        Some(
            "In Dynamic Trim, it plays from 2 seconds before the playhead to 2 seconds after \
             and puts the playhead back, or stops a trim. While Ripple Delete Silence is \
             open, it plays the timeline as Remove would leave it.",
        ),
    ),
    key(
        Play,
        &["J"],
        "Play backward",
        Some("Press J again to play backward faster."),
    ),
    key(
        Play,
        &["K"],
        "Stop",
        Some(
            "Hold K and tap J or L to step one frame. Hold K, then hold J or L, to play at half \
             speed until you let go.",
        ),
    ),
    key(
        Play,
        &["L"],
        "Play forward",
        Some(
            "Press L again to play faster. Editors keep three fingers on J, K and L to find a \
             cut by eye and ear.",
        ),
    ),
    key(Play, &["Left"], "One frame back", None),
    key(Play, &["Right"], "One frame forward", None),
    key(Play, &["Shift", "Left"], "One second back", None),
    key(Play, &["Shift", "Right"], "One second forward", None),
    key(
        Play,
        &["Up"],
        "Previous cut",
        Some("In Dynamic Trim, it applies the trim and trims that cut next."),
    ),
    key(
        Play,
        &["Down"],
        "Next cut",
        Some("In Dynamic Trim, it applies the trim and trims that cut next."),
    ),
    key(Play, &["Shift", "Up"], "Previous marker", None),
    key(Play, &["Shift", "Down"], "Next marker", None),
    key(Play, &["Home"], "Start of the timeline", None),
    key(Play, &["End"], "End of the timeline", None),
    key(
        Play,
        &["="],
        "Go to a timecode",
        Some("Type the digits after =. They fill from the right, so 100 goes to 00:00:01:00."),
    ),
    key(
        Play,
        &["+"],
        "Jump forward",
        Some("Type how far after +, as for =: 100 jumps one second."),
    ),
    key(
        Play,
        &["-"],
        "Jump back",
        Some("Type how far after -, as for =."),
    ),
    key(
        Play,
        &["Q"],
        "Timeline or source in the viewer",
        Some(
            "The viewer shows one thing at a time: the timeline, or the source you pick clips \
             from. Q switches to the one you looked at last.",
        ),
    ),
    key(
        Play,
        &["Shift", "S"],
        "Audio scrubbing on or off",
        Some(
            "With scrubbing on, you hear the sound under the playhead as you drag it or step \
             frames. It helps to find a cut on a word.",
        ),
    ),
    key(
        Play,
        &["Ctrl", "/"],
        "Loop on or off",
        Some(
            "Playing past the end goes on from the start. With both In and Out marked, the \
             part between them plays over and over.",
        ),
    ),
    key(
        Mark,
        &["I"],
        "Mark In",
        Some(
            "Marks pick a part. In the source they pick the part of a clip to edit in. In the \
             timeline they pick the stretch to replace or delete.",
        ),
    ),
    key(Mark, &["O"], "Mark Out", None),
    key(Mark, &["Shift", "I"], "Go to In", None),
    key(Mark, &["Shift", "O"], "Go to Out", None),
    key(Mark, &["Alt", "X"], "Clear In and Out", None),
    key(
        Mark,
        &["M"],
        "Add a marker",
        Some("On a marker, M opens it for editing."),
    ),
    key(Mark, &["Alt", "M"], "Delete the marker", None),
    key(
        Edit,
        &["F9"],
        "Smart Insert",
        Some(
            "Puts the source in at the cut nearest the playhead, so you never split a shot by \
             accident. On Track 1 everything after it moves right to make room.",
        ),
    ),
    key(
        Edit,
        &["F10"],
        "Overwrite",
        Some(
            "Writes the source over whatever is at the playhead. Nothing else moves, so the \
             timeline keeps its length.",
        ),
    ),
    key(
        Edit,
        &["Shift", "F10"],
        "Ripple Overwrite",
        Some(
            "Swaps the clip under the playhead for the source, even when they're different \
             lengths. Everything after it moves, so nothing is cut short or left over. Use it \
             to try a better take.",
        ),
    ),
    key(
        Edit,
        &["F12"],
        "Place On Top",
        Some(
            "Puts the source on a track above, lined up with the playhead, and leaves the \
             story underneath alone. Use it for cutaways and B-roll.",
        ),
    ),
    key(
        Edit,
        &["Shift", "F12"],
        "Append",
        Some("Adds the source after the last clip, wherever the playhead is."),
    ),
    key(
        Clips,
        &["Ctrl", "B"],
        "Split at the playhead",
        Some(
            "Ctrl+\\ does the same. It splits the selected clips and subtitles the playhead is \
             over, or with nothing selected, every clip there.",
        ),
    ),
    key(
        Clips,
        &["Ctrl", "T"],
        "Dissolve",
        Some(
            "A one-second dissolve on the selected cut, else on each cut of the selected clips, else \
             on Track 1's cut nearest the playhead: one picture fades out as the next fades in. Where the clips' sounds meet at the cut, \
             they cross fade. Next to a gap it fades from or to black. It is shorter where the clips have fewer frames past \
             the cut. U picks whether it ends on the cut, starts on it or sits across it.",
        ),
    ),
    key(
        Clips,
        &["Delete"],
        "Delete",
        Some(
            "Removes the selected clips and subtitles. On Track 1 the gap closes: Track 1 holds \
             the main story and never has holes. Backspace does the same. With a dissolve selected, it \
             removes the dissolve and its Cross Fade. After a click in the Media Pool, it \
             removes the selected media from the pool instead, after asking.",
        ),
    ),
    key(
        Clips,
        &["Alt", "Delete"],
        "Lift",
        Some("Removes the selected clips and leaves the gap, so nothing after them moves."),
    ),
    key(Clips, &["Ctrl", "C"], "Copy", None),
    key(
        Clips,
        &["Ctrl", "X"],
        "Cut",
        Some("Copies the selected clips and removes them. On Track 1 the gap closes."),
    ),
    key(
        Clips,
        &["Ctrl", "V"],
        "Paste",
        Some("Pastes at the playhead on the clips' own track, over what is there."),
    ),
    key(
        Clips,
        &["D"],
        "Picture off or on",
        Some("The clip stays in place with its picture off, so you see what's under it."),
    ),
    key(
        Clips,
        &["T"],
        "Transform",
        Some(
            "Shows the selected clip's handles in the viewer: drag inside to move it, a corner \
             to zoom, a side to stretch, the handle above to turn it. T again hides them.",
        ),
    ),
    key(
        Clips,
        &["Shift", "T"],
        "Crop",
        Some("Shows a handle on each side of the selected clip in the viewer to crop it."),
    ),
    key(
        Clips,
        &["Ctrl", "D"],
        "Change duration",
        Some("Of the selected clips, or of the selected dissolve, as a timecode or in frames."),
    ),
    key(
        Clips,
        &["R"],
        "Change speed",
        Some("A speed in percent, played backward, or one frame held still."),
    ),
    key(
        Clips,
        &["Enter"],
        "Edit the selected subtitle",
        Some("Opens its text and position in the Subtitle window; Ctrl+Enter there is Done."),
    ),
    key(
        Clips,
        &["]"],
        "Next silence stripe",
        Some("While Ripple Delete Silence is open: the playhead goes to the next stripe."),
    ),
    key(
        Clips,
        &["["],
        "Previous silence stripe",
        Some("While Ripple Delete Silence is open: the playhead goes to the stripe before."),
    ),
    key(
        Clips,
        &["X"],
        "Keep or cut the silence stripe at the playhead",
        Some(
            "While Ripple Delete Silence is open. A kept stripe turns gray and hatched and \
             stays when Remove cuts the red ones. A click on a stripe does the same, and its \
             edges drag to cut less or more.",
        ),
    ),
    key(
        Trim,
        &["Ctrl", "A"],
        "Select all",
        Some(
            "Every clip and subtitle on the timeline. After a click in the Media Pool, all it \
             shows.",
        ),
    ),
    key(Trim, &["Ctrl", "Shift", "A"], "Select nothing", None),
    key(
        Trim,
        &["Alt", "Shift", "V"],
        "Select the clips at the playhead",
        Some("Every clip under the playhead, on every track."),
    ),
    key(
        Trim,
        &["V"],
        "Select the nearest cut",
        Some("Selects the cut nearest the playhead, ready to trim with comma and period."),
    ),
    key(
        Trim,
        &["U"],
        "Switch the side of the cut",
        Some(
            "A cut has two sides: the end of the clip going out and the start of the one \
             coming in. U goes from rolling both, which moves the cut and keeps the length, \
             to the clip coming in, to the clip going out. In Dynamic Trim too.",
        ),
    ),
    key(
        Trim,
        &[","],
        "Trim the cut a frame left",
        Some("Hold Shift to trim a second."),
    ),
    key(
        Trim,
        &["."],
        "Trim the cut a frame right",
        Some("Hold Shift to trim a second."),
    ),
    key(
        Trim,
        &["Click"],
        "Trim Editor",
        Some(
            "Click a cut: while it is selected and the player is stopped, the viewer shows \
             the last frame going out beside the first frame coming in, over their \
             filmstrips. Drag a filmstrip's handle, or use the -1 and +1 buttons beside the \
             transport, to trim the clip going out, the one coming in, or both. Click a \
             dissolve to see it between the filmstrips and drag its ends to change its \
             length. Space plays as usual; Esc closes it.",
        ),
    ),
    key(
        Trim,
        &["W"],
        "Dynamic Trim",
        Some(
            "Trims with J, K and L. Press W on a selected cut, or on the cut nearest the \
             playhead: the playhead turns yellow and goes to the cut, and the Trim Editor \
             shows it. J and L now move the cut instead of the playhead, playing it with \
             sound while the viewer shows the frames on both sides, so you see and hear the \
             timing change and stop where it looks right. With clips selected instead, it slips or slides them: \
             right-click its button to choose. Press W again to finish.",
        ),
    ),
    key(
        Trim,
        &["L"],
        "Dynamic Trim forward",
        Some("Press L again to trim faster: 2, 4, 8, 16, 32 and 64 times."),
    ),
    key(
        Trim,
        &["J"],
        "Dynamic Trim backward",
        Some("Press J again to trim faster."),
    ),
    key(
        Trim,
        &["K"],
        "Dynamic Trim: stop",
        Some(
            "Stops the trim where it is. A trim also stops where a clip runs out: the cut turns \
             red and the viewer says why.",
        ),
    ),
    key(
        Trim,
        &["K", "L"],
        "Dynamic Trim one frame forward",
        Some("Hold L too to trim at half speed until you let go."),
    ),
    key(
        Trim,
        &["K", "J"],
        "Dynamic Trim one frame backward",
        Some("Hold J too to trim at half speed until you let go."),
    ),
    key(
        Trim,
        &["N"],
        "Snapping on or off",
        Some("Edges, markers and the playhead pull each other into line while you drag."),
    ),
    key(Trim, &["Ctrl", "Wheel"], "Zoom the timeline", None),
    key(
        App,
        &["Ctrl", "Z"],
        "Undo",
        Some("While Ripple Delete Silence is open, it undoes a change to its stripes or settings."),
    ),
    key(
        App,
        &["Ctrl", "Shift", "Z"],
        "Redo",
        Some("While Ripple Delete Silence is open, it redoes a change to its stripes or settings."),
    ),
    key(App, &["Ctrl", "I"], "Import media", None),
    key(App, &["Ctrl", "Shift", "N"], "New bin", None),
    key(App, &["Ctrl", ","], "Preferences", None),
    key(App, &["F1"], "Keyboard shortcuts", None),
];

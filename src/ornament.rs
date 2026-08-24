//! The hidden `--gp` ornament: a large gecko slowly eats a dolphin.
//!
//! The artwork is intentionally kept in the source rather than documented in
//! the public CLI help or README. The original signatures are retained in the
//! artwork.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyModifiers},
    execute, queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

const FRAME_DELAY: Duration = Duration::from_millis(600);

// Large Tokee gecko artwork.
const GECKO_ART: &str = r##"                           __,---'::.-  -::_ _ `-----.___      ______
                       _,-'::_  ::-  -  -. _   ::-::_   .`--,'   :: .:`-._
                    ,-'_ ::   _  ::_ .:   :: - _ .:   ::- _/ ::   ,-. ::. `-._
                _,-'   ::-  ::        ::-  _ ::  -  ::     |  .: ((|))      ::`.
        ___,---'   ::    ::    ;::   ::     :.- _ ::._  :: \ :    `_____::..--'
    ,-""  ::  ::.   ,------.  (.  ::  \  ::  ::  ,-- :. _  :`. ::  \       `-._
  ,'     ::   '   _._.:_  :.)___,-------------._(.:: ____`-._ `._ ::`--...___; ;
 ;:::. ,--'--"""""      /  /                     \. |     ``-----`''`---------'
;  `::;              _ /.:/_,                    _\.:\_,
|    ;            ='-//\\--"                  ='-//\\--"
`   .|               ''  ``                      ''  ``
 \::'\
  \   \
   `..:`.
     `.  `--.____
       `-:______ `-._
                `---'"##;

// Small dolphin artwork, sized to fit at the gecko's mouth. Keep this as
// explicit lines so the asymmetric leading spaces and backslashes cannot be
// lost to reformatting.
const DOLPHIN_ART: [&str; 6] = [
    r"                  _.-,",
    r"              .--'  '-._",
    r"           _/`-  _      '.",
    r"          '----'._`.----. \",
    r"                   `     \;",
    r"                         ;_\",
];

// The two final lines alternate while the dolphin swims. Keeping the body
// identical makes the caudal fin the only moving part of the sprite.
const DOLPHIN_ART_FIN_UP: [&str; 6] = [
    DOLPHIN_ART[0],
    DOLPHIN_ART[1],
    DOLPHIN_ART[2],
    DOLPHIN_ART[3],
    r"                   `     /;",
    r"                         ;/",
];

const ART_CREDITS: [&str; 3] = [
    "ASCII art courtesy of:",
    "  gecko   - jrei",
    "  dolphin - jgs",
];

const DEDICATION: &str = "For GP, Danny Levin Award 2026 winner, with love from the Kupala team";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AnimationFrame {
    /// Horizontal position of the dolphin relative to the gecko's left edge.
    dolphin_x: i16,
    /// Vertical position of the dolphin relative to the gecko's top edge.
    dolphin_y: u16,
    /// Number of dolphin rows still visible. `None` means it has been eaten.
    dolphin_rows: Option<usize>,
    /// Left boundary used to hide the portion already inside the gecko.
    dolphin_clip_left: Option<i16>,
    /// Whether the dolphin's caudal fin is raised.
    dolphin_fin_up: bool,
}

const DOLPHIN_START_OFFSET: i16 = 42;
const DOLPHIN_Y: u16 = 2;
// The supplied dolphin's body leading edge begins at column 10.
const DOLPHIN_NOSE_OFFSET: i16 = 10;
const DOLPHIN_NEAR_MOUTH_GAP: i16 = 12;
const CYCLE_FRAMES: usize = 32;

/// Run the hidden ornament until the user presses a key or Ctrl-C.
pub fn run() -> anyhow::Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Hide)?;
    if let Err(error) = terminal::enable_raw_mode() {
        let _ = execute!(stdout, Show, LeaveAlternateScreen);
        return Err(error.into());
    }

    let result = animate(&mut stdout);

    // Always restore the user's terminal, including when rendering or input
    // handling returns an error.
    let restore_result = terminal::disable_raw_mode();
    let leave_result = execute!(stdout, Show, LeaveAlternateScreen);
    result?;
    restore_result?;
    leave_result?;
    Ok(())
}

fn animate(stdout: &mut io::Stdout) -> anyhow::Result<()> {
    let mut frame_index = 0;
    loop {
        render_frame(stdout, animation_frame(frame_index))?;
        stdout.flush()?;

        let deadline = Instant::now() + FRAME_DELAY;
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if event::poll(remaining.min(Duration::from_millis(50)))?
                && let Event::Key(key) = event::read()?
                && (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
                    || matches!(key.code, KeyCode::Char('q') | KeyCode::Esc))
            {
                return Ok(());
            }
        }
        frame_index += 1;
    }
}

fn animation_frame(index: usize) -> AnimationFrame {
    let gecko_width = art_width(GECKO_ART) as i16;
    let dolphin_rows = DOLPHIN_ART.len();
    let mouth_x = gecko_width.saturating_sub(10);
    // The dolphin faces left: its first visible character is about ten cells
    // into the sprite. Align that nose with the gecko's mouth on contact.
    // Contact position: the dolphin's nose reaches the gecko's mouth.
    let contact_x = mouth_x - DOLPHIN_NOSE_OFFSET;
    let start_x = gecko_width + DOLPHIN_START_OFFSET;
    // Keep the complete dolphin outside the gecko until the contact frame.
    // This avoids drawing its leading point over the head while it approaches.
    let near_mouth_x = contact_x + DOLPHIN_NEAR_MOUTH_GAP;
    let approach_span = start_x - near_mouth_x;
    // There are sixteen approach frames followed by an explicit near-mouth
    // frame. Use a denominator of sixteen so the last interpolated frame does
    // not round to the same column as that next frame.
    let approach_x = |step: i16| start_x - approach_span * step / 16;
    let phase = index % CYCLE_FRAMES;

    match phase {
        // Sixteen measured steps make the approach visible without moving the
        // gecko or shifting the whole composition on each frame.
        0..=15 => AnimationFrame {
            dolphin_x: approach_x(phase as i16),
            dolphin_y: DOLPHIN_Y,
            dolphin_rows: Some(dolphin_rows),
            dolphin_clip_left: None,
            dolphin_fin_up: phase % 2 == 1,
        },
        16 => AnimationFrame {
            dolphin_x: near_mouth_x,
            dolphin_y: DOLPHIN_Y,
            dolphin_rows: Some(dolphin_rows),
            dolphin_clip_left: None,
            dolphin_fin_up: false,
        },
        17 => AnimationFrame {
            dolphin_x: contact_x,
            dolphin_y: DOLPHIN_Y,
            dolphin_rows: Some(dolphin_rows),
            dolphin_clip_left: Some(gecko_width),
            dolphin_fin_up: false,
        },
        18..=25 => AnimationFrame {
            // Move the dolphin into the mouth one column per frame. Clip the
            // part behind the mouth so it cannot appear on the gecko's head
            // or emerge on the far side.
            dolphin_x: contact_x - (phase as i16 - 17),
            dolphin_y: DOLPHIN_Y,
            dolphin_rows: Some(dolphin_rows),
            dolphin_clip_left: Some(gecko_width),
            dolphin_fin_up: false,
        },
        // Hold the empty mouth briefly before the next dolphin arrives.
        26..=31 => AnimationFrame {
            dolphin_x: contact_x,
            dolphin_y: DOLPHIN_Y,
            dolphin_rows: None,
            dolphin_clip_left: None,
            dolphin_fin_up: false,
        },
        _ => unreachable!(),
    }
}

fn render_frame(stdout: &mut io::Stdout, frame: AnimationFrame) -> anyhow::Result<()> {
    let (width, height) = terminal::size()?;
    let gecko_lines: Vec<&str> = GECKO_ART.lines().collect();
    let dolphin_lines = if frame.dolphin_fin_up {
        &DOLPHIN_ART_FIN_UP
    } else {
        &DOLPHIN_ART
    };
    let gecko_width = art_width(GECKO_ART) as i16;
    // Center against the widest fin pose, not the currently selected sprite;
    // otherwise the gecko shifts sideways every time the fin changes.
    let dolphin_width = dolphin_scene_width();
    // Keep the gecko fixed while the dolphin approaches. The scene width is
    // based on the first (furthest-away) pose rather than the current pose,
    // otherwise the whole composition shifts as the dolphin moves.
    let scene_width = gecko_width + DOLPHIN_START_OFFSET + dolphin_width;
    let dedication_lines = [DEDICATION];
    let artwork_height = gecko_lines.len() as u16;
    let credits_height = 1 + dedication_lines.len() as u16 + 1 + ART_CREDITS.len() as u16;
    let scene_height = artwork_height.max(
        frame
            .dolphin_rows
            .map(|rows| frame.dolphin_y + rows.min(dolphin_lines.len()) as u16)
            .unwrap_or(0),
    ) + credits_height;
    let origin_x = (width as i16 - scene_width).max(0) / 2;
    let origin_y = (height.saturating_sub(scene_height)) / 2;

    queue!(stdout, Clear(ClearType::All))?;
    if let Some(rows) = frame.dolphin_rows {
        draw_art(
            stdout,
            &dolphin_lines[..rows.min(dolphin_lines.len())],
            origin_x + frame.dolphin_x,
            origin_y + frame.dolphin_y,
            width,
            height,
            Color::Cyan,
            frame.dolphin_clip_left.map(|x| origin_x + x),
        )?;
    }
    // Paint the gecko after the dolphin so it naturally occludes the
    // swallowed portion. The dolphin is clipped explicitly to the gecko's
    // mouth before it is drawn, because sparse ASCII spaces are transparent.
    draw_art(
        stdout,
        &gecko_lines,
        origin_x,
        origin_y,
        width,
        height,
        Color::Green,
        None,
    )?;
    let dedication_x = origin_x + (gecko_width - sprite_width(&dedication_lines) as i16).max(0) / 2;
    draw_art(
        stdout,
        &dedication_lines,
        dedication_x,
        origin_y + artwork_height + 1,
        width,
        height,
        Color::Yellow,
        None,
    )?;
    let credits_x = origin_x + (gecko_width - sprite_width(&ART_CREDITS) as i16).max(0) / 2;
    draw_art(
        stdout,
        &ART_CREDITS,
        credits_x,
        origin_y + artwork_height + 3,
        width,
        height,
        Color::DarkGrey,
        None,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_art(
    stdout: &mut io::Stdout,
    lines: &[&str],
    x: i16,
    y: u16,
    width: u16,
    height: u16,
    color: Color,
    clip_left: Option<i16>,
) -> anyhow::Result<()> {
    queue!(stdout, SetForegroundColor(color))?;
    for (row, line) in lines.iter().enumerate() {
        let row = y.saturating_add(row as u16);
        if row >= height {
            break;
        }
        for (column, character) in line.chars().enumerate() {
            if character == ' ' {
                continue;
            }
            let column = x + column as i16;
            if clip_left.is_some_and(|left| column < left) {
                continue;
            }
            if column >= 0 && (column as u16) < width {
                queue!(stdout, MoveTo(column as u16, row), Print(character))?;
            }
        }
    }
    queue!(stdout, ResetColor)?;
    Ok(())
}

fn art_width(art: &str) -> usize {
    art.lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
}

fn sprite_width(lines: &[&str]) -> usize {
    lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
}

fn dolphin_scene_width() -> i16 {
    sprite_width(&DOLPHIN_ART).max(sprite_width(&DOLPHIN_ART_FIN_UP)) as i16
}

#[cfg(test)]
mod tests {
    use super::{
        ART_CREDITS, AnimationFrame, CYCLE_FRAMES, DEDICATION, DOLPHIN_ART, DOLPHIN_ART_FIN_UP,
        DOLPHIN_NOSE_OFFSET, GECKO_ART, animation_frame, art_width, dolphin_scene_width,
        sprite_width,
    };

    #[test]
    fn animation_has_approach_eating_and_empty_poses() {
        assert!(animation_frame(0).dolphin_rows.is_some());
        assert!(animation_frame(15).dolphin_x > animation_frame(17).dolphin_x);
        assert!(animation_frame(15).dolphin_x > animation_frame(16).dolphin_x);
        assert!(animation_frame(18).dolphin_x < animation_frame(17).dolphin_x);
        assert_eq!(
            animation_frame(18).dolphin_clip_left,
            Some(art_width(GECKO_ART) as i16)
        );
        assert!(!animation_frame(16).dolphin_fin_up);
        assert!(!animation_frame(17).dolphin_fin_up);
        assert_eq!(animation_frame(26).dolphin_rows, None);
    }

    #[test]
    fn dolphin_caudal_fin_alternates_during_approach_only() {
        assert!(!animation_frame(0).dolphin_fin_up);
        assert!(animation_frame(1).dolphin_fin_up);
        assert!(!animation_frame(16).dolphin_fin_up);
        assert!(!animation_frame(17).dolphin_fin_up);
        assert!(!animation_frame(26).dolphin_fin_up);
    }

    #[test]
    fn fin_poses_use_one_stable_scene_width() {
        let down_width = sprite_width(&DOLPHIN_ART) as i16;
        let up_width = sprite_width(&DOLPHIN_ART_FIN_UP) as i16;
        assert_eq!(dolphin_scene_width(), down_width.max(up_width));
    }

    #[test]
    fn animation_restarts_at_the_same_position_every_cycle() {
        assert_eq!(animation_frame(0), animation_frame(CYCLE_FRAMES));
        assert_eq!(animation_frame(1), animation_frame(CYCLE_FRAMES + 1));
        assert_eq!(animation_frame(31).dolphin_rows, None);
        assert!(animation_frame(CYCLE_FRAMES).dolphin_rows.is_some());
    }

    #[test]
    fn supplied_art_has_expected_dimensions() {
        assert!(GECKO_ART.lines().count() >= 15);
        assert_eq!(DOLPHIN_ART.len(), 6);
        assert!(art_width(GECKO_ART) > 60);
        assert!(sprite_width(&DOLPHIN_ART) < 35);
        assert_eq!(DOLPHIN_ART[0], "                  _.-,");
        assert_eq!(DOLPHIN_ART[1], "              .--'  '-._");
        assert_eq!(DOLPHIN_ART[4], "                   `     \\;");
        assert_eq!(DOLPHIN_ART[5], "                         ;_\\");
    }

    #[test]
    fn dolphin_reaches_the_gecko_mouth() {
        let gecko_width = art_width(GECKO_ART) as i16;
        let mouth_x = gecko_width - 10;
        let eaten = animation_frame(17);
        assert_eq!(eaten.dolphin_x, mouth_x - DOLPHIN_NOSE_OFFSET);
        assert_eq!(eaten.dolphin_y, 2);
    }

    #[test]
    fn animation_frame_is_copyable_and_comparable() {
        let frame = AnimationFrame {
            dolphin_x: 1,
            dolphin_y: 2,
            dolphin_rows: None,
            dolphin_clip_left: None,
            dolphin_fin_up: false,
        };
        assert_eq!(frame, frame);
    }

    #[test]
    fn credits_keep_the_original_artists_outside_the_sprites() {
        assert_eq!(ART_CREDITS[0], "ASCII art courtesy of:");
        assert_eq!(ART_CREDITS[1], "  gecko   - jrei");
        assert_eq!(ART_CREDITS[2], "  dolphin - jgs");
        assert!(!GECKO_ART.contains("jrei"));
        assert!(!DOLPHIN_ART.iter().any(|line| line.contains("jgs")));
    }

    #[test]
    fn dedication_names_the_award_winner() {
        assert_eq!(
            DEDICATION,
            "For GP, Danny Levin Award 2026 winner, with love from the Kupala team"
        );
        assert!(!GECKO_ART.starts_with("Tokee"));
        assert!(!GECKO_ART.contains("Gecko gekko"));
    }
}

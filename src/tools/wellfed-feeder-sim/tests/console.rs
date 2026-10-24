//! The operator's console lines, as the gestures the button service would
//! send for the same hands.

use wellfed_core::buttons::{Button, Buttons, Gesture};
use wellfed_feeder_sim::console::{ConsoleError, parse};
use wellfed_feeder_sim::serve::Control;

fn gestures(line: &str) -> Vec<Gesture> {
    parse(line)
        .unwrap()
        .into_iter()
        .map(|control| match control {
            Control::Gesture(gesture) => gesture,
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn a_press_is_a_down_then_an_up_by_name_or_position() {
    let by_name = gestures("press lock mic");
    let by_position = gestures("press 1 3");
    assert_eq!(by_name, by_position);

    let lock = Buttons::of(Button::Lock);
    assert_eq!(by_name[0], Gesture::Down { chord: lock });
    assert!(matches!(
        by_name[1],
        Gesture::Up { chord, level: 0, .. } if chord == lock
    ));
    assert_eq!(by_name.len(), 4);
}

#[test]
fn a_hold_builds_its_chord_then_holds_past_the_first_level() {
    let chord = Buttons::of(Button::Lock).with(Button::Dispense);
    assert_eq!(
        gestures("hold lock+dispense")[..3],
        [
            Gesture::Down {
                chord: Buttons::of(Button::Lock)
            },
            Gesture::Down { chord },
            Gesture::Hold { chord, level: 1 },
        ]
    );
}

#[test]
fn other_lines_are_refused_or_ignored() {
    assert_eq!(parse("   "), Ok(Vec::new()));
    assert_eq!(parse("reboot"), Ok(vec![Control::Reboot]));
    assert_eq!(parse("press"), Err(ConsoleError::NoButtons));
    assert_eq!(parse("hold +"), Err(ConsoleError::NoButtons));
    assert_eq!(parse("press 5"), Err(ConsoleError::Button("5".to_owned())));
    assert_eq!(parse("feed"), Err(ConsoleError::Command("feed".to_owned())));
}

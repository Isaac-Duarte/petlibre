//! The audio handle's contract against the fake codec ring: one handle at a
//! time, one page at a time, and a page that is not submitted stays free.

use wellfed_amebad_sys::{AUDIO_PAGES, Audio, audio_close, audio_open, fake};

fn queued() -> usize {
    fake::with(|h| h.audio_queue.len())
}

#[test]
fn one_handle_at_a_time_and_a_drop_releases_it() {
    fake::reset_hardware();

    let first = Audio::take().expect("the first take gets the handle");
    assert!(Audio::take().is_none(), "a second handle while one is out");

    drop(first);
    assert!(Audio::take().is_some(), "dropping the handle releases it");
}

#[test]
fn an_unsubmitted_page_stays_free_and_a_submitted_one_queues() {
    fake::reset_hardware();
    assert!(audio_open(16_000));
    let mut audio = Audio::take().expect("handle");

    let address = {
        let mut page = audio.page().expect("a free page");
        page.fill(7);
        page.as_ptr()
    };
    assert_eq!(queued(), 0, "a dropped page is not handed to the DMA");

    let mut page = audio.page().expect("the same page again");
    assert_eq!(page.as_ptr(), address);
    page.fill(9);
    page.submit();
    assert_eq!(queued(), 1);
    assert!(fake::with(|h| h.audio_queue[0].iter().all(|&s| s == 9)));

    audio_close();
}

#[test]
fn no_page_while_closed_or_full() {
    fake::reset_hardware();
    let mut audio = Audio::take().expect("handle");
    assert!(audio.page().is_none(), "the codec is closed");

    assert!(audio_open(16_000));
    for _ in 0..AUDIO_PAGES {
        audio.page().expect("a free page").submit();
    }
    assert!(audio.page().is_none(), "the DMA owns every page");

    audio_close();
}

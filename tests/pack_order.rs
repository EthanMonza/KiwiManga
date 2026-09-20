//! Packing order: zero-padding, continuous merged numbering, per-chapter layout.

use kiwimanga::pack;
use kiwimanga::sources::PageData;
use std::io::Cursor;

fn page(byte: u8, ext: &str) -> PageData {
    PageData { ext: ext.to_string(), bytes: vec![byte; 16] }
}

fn names(bytes: &[u8]) -> Vec<String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect()
}

#[test]
fn single_chapter_zero_padded() {
    let pages: Vec<PageData> =
        (0..5u8).map(|b| page(b, if b % 2 == 0 { "jpg" } else { "png" })).collect();
    let bytes = pack::pack_manga_chapter(&pages).unwrap();
    assert_eq!(names(&bytes), vec!["001.jpg", "002.png", "003.jpg", "004.png", "005.jpg"]);
}

#[test]
fn merged_is_one_continuous_sequence() {
    let ch1: Vec<PageData> = (0..120u8).map(|b| page(b, "jpg")).collect();
    let ch2: Vec<PageData> = vec![page(200, "png"), page(201, "jpg")];
    let bytes = pack::pack_manga_merged(&[ch1, ch2]).unwrap();
    let got = names(&bytes);
    assert_eq!(got.len(), 122);
    assert_eq!(got[0], "001.jpg");
    assert_eq!(got[119], "120.jpg");
    assert_eq!(got[120], "121.png");
    assert_eq!(got[121], "122.jpg");
    // Width grows with size: 1234 pages -> 0001..1234.
    let big: Vec<PageData> = (0..1234).map(|i| page((i % 251) as u8, "jpg")).collect();
    let bytes = pack::pack_manga_chapter(&big).unwrap();
    let got = names(&bytes);
    assert_eq!(got[0], "0001.jpg");
    assert_eq!(got[1233], "1234.jpg");
}

#[test]
fn per_chapter_has_n_inner_files() {
    let chs = vec![
        ("ch 1".to_string(), vec![page(1, "jpg")]),
        ("ch 2".to_string(), vec![page(2, "jpg"), page(3, "png")]),
        ("ch 3".to_string(), vec![page(4, "jpg")]),
    ];
    let bytes = pack::pack_manga_per_chapter(&chs).unwrap();
    assert_eq!(names(&bytes), vec!["ch 1.cbz", "ch 2.cbz", "ch 3.cbz"]);
    // Inner files are valid zips with ordered pages.
    let mut outer = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let mut inner_raw = Vec::new();
    std::io::Read::read_to_end(&mut outer.by_index(1).unwrap(), &mut inner_raw).unwrap();
    assert_eq!(names(&inner_raw), vec!["001.jpg", "002.png"]);
}

#[test]
fn ranobe_volume_builds_epub() {
    let chs = vec![
        ("Ch. 1".to_string(), vec!["First.".to_string()]),
        ("Ch. 2".to_string(), vec!["Second.".to_string(), "More.".to_string()]),
    ];
    let bytes = pack::pack_ranobe_volume("Novel", &chs).unwrap();
    assert_eq!(&bytes[0..2], b"PK");
    // EPUB container files present.
    let got = names(&bytes);
    assert!(got.iter().any(|n| n == "mimetype"), "mimetype missing: {got:?}");
    assert!(got.iter().any(|n| n.contains("ch000")), "chapters missing: {got:?}");
}

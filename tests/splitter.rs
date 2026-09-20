//! 50MB splitter: greedy chapter-range parts + oversize detection.

use kiwimanga::pack::plan_parts;

const MB: u64 = 1024 * 1024;

#[test]
fn fits_in_one_part() {
    let (parts, over) = plan_parts(&[10 * MB, 20 * MB], 50 * MB);
    assert_eq!(parts, vec![vec![0, 1]]);
    assert!(over.is_empty());
}

#[test]
fn exact_boundary_fits() {
    let (parts, over) = plan_parts(&[25 * MB, 25 * MB], 50 * MB);
    assert_eq!(parts, vec![vec![0, 1]]);
    assert!(over.is_empty());
}

#[test]
fn greedy_fill() {
    // 20+20 fits, third 20 overflows -> new part.
    let (parts, over) = plan_parts(&[20 * MB, 20 * MB, 20 * MB], 50 * MB);
    assert_eq!(parts, vec![vec![0, 1], vec![2]]);
    assert!(over.is_empty());
}

#[test]
fn many_small_chapters_pack_tightly() {
    let sizes = vec![6 * MB; 20]; // 120MB total
    let (parts, over) = plan_parts(&sizes, 50 * MB);
    assert!(over.is_empty());
    assert_eq!(parts.len(), 3); // 8+8+4 chapters
    assert_eq!(parts[0].len(), 8);
    assert_eq!(parts[1].len(), 8);
    assert_eq!(parts[2].len(), 4);
    // Order preserved, no chapter lost or duplicated.
    let flat: Vec<usize> = parts.concat();
    assert_eq!(flat, (0..20).collect::<Vec<_>>());
}

#[test]
fn oversize_chapter_flagged_but_isolated() {
    let (parts, over) = plan_parts(&[10 * MB, 99 * MB, 10 * MB], 50 * MB);
    assert_eq!(over, vec![1]);
    assert_eq!(parts, vec![vec![0], vec![1], vec![2]]);
}

//! An upload arena's growth policy, independent of the address allocator.
#[test]
fn small_request_still_fits_when_preferred_arena_does_not() {
    let mut calls = Vec::new();
    let got = grow(3, 64, |n| {
        calls.push(n);
        (n == 3).then_some(123)
    });
    assert_eq!(got, Some((123, 3)));
    assert_eq!(calls, [64, 3]);
}

#[test]
fn prefer_reusable_arena_and_never_retry_the_same_size() {
    assert_eq!(grow(3, 64, |_| Some(123)), Some((123, 64)));
    let mut calls = Vec::new();
    assert_eq!(
        grow(65, 64, |n| {
            calls.push(n);
            None
        }),
        None
    );
    assert_eq!(calls, [65]);
}

pub(super) fn grow(
    required: usize,
    typical: usize,
    mut reserve: impl FnMut(usize) -> Option<usize>,
) -> Option<(usize, usize)> {
    let blocks = required.max(typical);
    reserve(blocks).map(|at| (at, blocks)).or_else(|| {
        if blocks == required {
            None
        } else {
            reserve(required).map(|at| (at, required))
        }
    })
}

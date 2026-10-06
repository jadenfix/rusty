use ring::RingBuffer;
use std::collections::VecDeque;

#[test]
fn overwriting_returns_the_oldest() {
    let mut r = RingBuffer::new(2);
    assert_eq!(r.push('a'), None);
    assert_eq!(r.push('b'), None);
    assert_eq!(r.push('c'), Some('a'));
    assert_eq!(r.iter().collect::<String>(), "bc");
}

#[test]
fn pops_across_the_wrap() {
    let mut r = RingBuffer::new(3);
    for i in 0..10 {
        r.push(i);
        if i % 2 == 0 {
            r.pop();
        }
    }
    let mut out = Vec::new();
    while let Some(x) = r.pop() {
        out.push(x);
    }
    assert!(r.is_empty() && r.pop().is_none());
    assert_eq!(out, vec![7, 8, 9]);
}

#[test]
fn matches_a_deque_on_a_long_mixed_run() {
    for cap in 1..6 {
        let mut r = RingBuffer::new(cap);
        let mut model: VecDeque<u32> = VecDeque::new();
        let mut x: u32 = 12345;
        for step in 0..500u32 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            if x % 3 == 0 {
                assert_eq!(r.pop(), model.pop_front(), "cap {cap} step {step}");
            } else {
                let evicted = if model.len() == cap { model.pop_front() } else { None };
                model.push_back(step);
                assert_eq!(r.push(step), evicted, "cap {cap} step {step}");
            }
            assert_eq!(r.len(), model.len());
            assert_eq!(r.iter().copied().collect::<Vec<_>>(), model.iter().copied().collect::<Vec<_>>());
        }
    }
}

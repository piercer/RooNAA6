use std::collections::VecDeque;

use crate::proxy::{execute_ops, FrameOp};

fn run(ops: &mut VecDeque<FrameOp>, data: &[u8]) -> (Vec<u8>, usize) {
    let mut out = Vec::new();
    let mut pos = 0;
    execute_ops(ops, data, &mut pos, &mut out);
    (out, pos)
}

#[test]
fn pass_streams_bytes() {
    let mut ops = VecDeque::from(vec![FrameOp::Pass(5)]);
    let (out, pos) = run(&mut ops, b"hello world");
    assert_eq!(out, b"hello");
    assert_eq!(pos, 5);
    assert!(ops.is_empty());
}

#[test]
fn skip_discards_bytes() {
    let mut ops = VecDeque::from(vec![FrameOp::Skip(3), FrameOp::Pass(4)]);
    let (out, pos) = run(&mut ops, b"abcdefgh");
    assert_eq!(out, b"defg");
    assert_eq!(pos, 7);
    assert!(ops.is_empty());
}

#[test]
fn emit_writes_immediately() {
    let mut ops = VecDeque::from(vec![FrameOp::Emit(b"HELLO".to_vec())]);
    let (out, pos) = run(&mut ops, b"");
    assert_eq!(out, b"HELLO");
    assert_eq!(pos, 0);
    assert!(ops.is_empty());
}

#[test]
fn mixed_sequence() {
    let mut ops = VecDeque::from(vec![
        FrameOp::Pass(2),
        FrameOp::Emit(b"[X]".to_vec()),
        FrameOp::Skip(2),
        FrameOp::Pass(2),
    ]);
    let (out, pos) = run(&mut ops, b"ABCDEF");
    assert_eq!(out, b"AB[X]EF");
    assert_eq!(pos, 6);
    assert!(ops.is_empty());
}

#[test]
fn partial_pass_leaves_remainder_on_queue() {
    let mut ops = VecDeque::from(vec![FrameOp::Pass(10), FrameOp::Emit(b"!".to_vec())]);
    let (out, pos) = run(&mut ops, b"abcd");
    assert_eq!(out, b"abcd");
    assert_eq!(pos, 4);
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0], FrameOp::Pass(6));
}

#[test]
fn partial_skip_leaves_remainder_on_queue() {
    let mut ops = VecDeque::from(vec![FrameOp::Skip(10), FrameOp::Pass(3)]);
    let (out, pos) = run(&mut ops, b"abcd");
    assert!(out.is_empty());
    assert_eq!(pos, 4);
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0], FrameOp::Skip(6));
}

#[test]
fn empty_queue_is_noop() {
    let mut ops: VecDeque<FrameOp> = VecDeque::new();
    let (out, pos) = run(&mut ops, b"data");
    assert!(out.is_empty());
    assert_eq!(pos, 0);
}

#[test]
fn resumes_after_blocking() {
    let mut ops = VecDeque::from(vec![FrameOp::Pass(5), FrameOp::Emit(b"!".to_vec())]);

    let mut out1 = Vec::new();
    let mut pos1 = 0;
    execute_ops(&mut ops, b"abc", &mut pos1, &mut out1);
    assert_eq!(out1, b"abc");
    assert_eq!(pos1, 3);
    assert_eq!(ops.len(), 2);
    assert_eq!(ops[0], FrameOp::Pass(2));

    let mut out2 = Vec::new();
    let mut pos2 = 0;
    execute_ops(&mut ops, b"de", &mut pos2, &mut out2);
    assert_eq!(out2, b"de!");
    assert_eq!(pos2, 2);
    assert!(ops.is_empty());
}

#[test]
fn zero_length_ops_drain_without_blocking() {
    let mut ops = VecDeque::from(vec![
        FrameOp::Pass(0),
        FrameOp::Skip(0),
        FrameOp::Emit(Vec::new()),
        FrameOp::Pass(3),
    ]);
    let (out, pos) = run(&mut ops, b"xyz");
    assert_eq!(out, b"xyz");
    assert_eq!(pos, 3);
    assert!(ops.is_empty());
}

//! Structural checks: every instruction the decoder accepts lowers to IR the verifier accepts, or says why not. What
//! the IR computes is checked against Unicorn (tests/test_lifter_semantics.py).

use super::*;
use mir::{Function, Term};

fn xorshift(state: &mut u64) -> u32 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    (state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32
}

#[test]
fn every_decoded_word_lowers_to_valid_ir() {
    let mut state = 0x1234_5678_9abc_def1u64;
    let (mut lowered, mut unsupported, mut undefined) = (0, 0, 0);
    for _ in 0..400_000 {
        let w = xorshift(&mut state);
        let Some(insn) = a64::decode(w) else { continue };
        let mut f = Function::new(0x8000_0000);
        let b = f.start;
        match lower(&mut f, b, 0x8000_0000, &insn) {
            Ok(()) => {
                lowered += 1;
                f.terminate(b, Term::Return);
                if let Err(e) = f.verify() {
                    panic!("{w:#010x} ({insn}): {e}\n{f}");
                }
            }
            Err(Fault::Unsupported(_)) => unsupported += 1,
            Err(Fault::Undefined) => undefined += 1,
        }
    }
    assert!(lowered > 50_000, "{lowered} lowered, {unsupported} unsupported, {undefined} undefined");
}

/// The same over the floating-point and Advanced SIMD groups (words with their group bits set).
#[test]
fn every_simd_word_lowers_to_valid_ir() {
    let mut state = 0x0bad_5eed_1234_5678u64;
    let groups: [(u32, u32); 6] = [
        (0x1e00_0000, 0x0e00_0000),
        (0x5f00_0000, 0x1e00_0000),
        (0x5f00_0000, 0x1f00_0000),
        (0xbf00_0000, 0x0c00_0000),
        (0xbf00_0000, 0x0d00_0000),
        (0x3a00_0000, 0x2c00_0000),
    ];
    let mut lowered = 0;
    for &(mask, value) in &groups {
        for _ in 0..100_000 {
            let w = (xorshift(&mut state) & !mask) | value;
            let Some(insn) = a64::decode(w) else { continue };
            let mut f = Function::new(0x8000_0000);
            let b = f.start;
            if lower(&mut f, b, 0x8000_0000, &insn).is_ok() {
                lowered += 1;
                f.terminate(b, Term::Return);
                if let Err(e) = f.verify() {
                    panic!("{w:#010x} ({insn}): {e}\n{f}");
                }
            }
        }
    }
    assert!(lowered > 100_000, "{lowered} lowered");
}

#[test]
fn add_immediate_reads_and_writes_the_right_registers() {
    // add x0, sp, #16
    let insn = a64::decode(0x910043e0).unwrap();
    let mut f = Function::new(0x8000_0000);
    let b = f.start;
    lower(&mut f, b, 0x8000_0000, &insn).unwrap();
    f.terminate(b, Term::Return);
    let text = f.to_string();
    assert!(text.contains("get sp") && text.contains("set x0"), "{text}");
}

/// `w` (which reads `what`) lowered into a function of its own, verified: its text, or the fault.
fn lowered(w: u32, what: &str) -> Result<String, Fault> {
    let insn = a64::decode(w).expect("a word that decodes");
    assert_eq!(insn.to_string(), what);
    let mut f = Function::new(0x8000_0000);
    let b = f.start;
    lower(&mut f, b, 0x8000_0000, &insn)?;
    f.terminate(b, Term::Return);
    f.verify().unwrap();
    Ok(f.to_string())
}

#[test]
fn fp16_arithmetic_is_computed_in_single_precision() {
    for (w, what) in [
        (0x1ee2_2820, "fadd h0, h1, h2"),
        (0x1ee2_1820, "fdiv h0, h1, h2"),
        (0x1ee1_c020, "fsqrt h0, h1"),
        (0x4e42_1420, "fadd v0.8h, v1.8h, v2.8h"),
        (0x6e42_1c20, "fmul v0.8h, v1.8h, v2.8h"),
        (0x4e30_f820, "fmaxv h0, v1.8h"),
    ] {
        let text = lowered(w, what).unwrap_or_else(|e| panic!("{what}: {e:?}"));
        assert!(text.contains("fext") && text.contains("ftrunc"), "{what}: no half through single\n{text}");
    }
    // (the fused, estimate and fixed-point forms of halves stay uncovered: the guest stops at them)
    for (w, what) in [
        (0x1fc2_0c20, "fmadd h0, h1, h2, h3"),
        (0x4e42_0c20, "fmla v0.8h, v1.8h, v2.8h"),
        (0x5ef9_d820, "frecpe h0, h1"),
        (0x1ec2_fc20, "scvtf h0, w1, #1"),
    ] {
        assert!(matches!(lowered(w, what), Err(Fault::Unsupported(_))), "{what}");
    }
}

#[test]
fn dczid_el0_says_the_block_dc_zva_zeroes() {
    // mrs x0, dczid_el0: BS 4 (16 words, 64 bytes), DZP clear
    let text = lowered(0xd53b_00e0, "mrs x0, DCZID_EL0").unwrap();
    assert!(text.contains("const i64 0x4") && text.contains("set x0"), "{text}");
    // dc zva, x1: eight 8-byte stores from the 64-byte block's start
    let text = lowered(0xd50b_7421, "sys #3, c7, c4, #1, x1").unwrap();
    assert_eq!(text.matches("store").count(), 8, "{text}");
    assert!(text.contains("0xffffffffffffffc0"), "{text}");
}

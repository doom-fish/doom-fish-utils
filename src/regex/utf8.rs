#![allow(clippy::cast_possible_truncation)]

fn encode(value: u32, out: &mut [u8; 4]) -> usize {
    char::from_u32(value).map_or(0, |c| c.encode_utf8(out).len())
}

pub fn sequences(lo: u32, hi: u32, out: &mut Vec<Vec<(u8, u8)>>) {
    let mut stack = vec![(lo, hi)];
    while let Some((lo, mut hi)) = stack.pop() {
        'split: loop {
            if lo > hi {
                break 'split;
            }
            if lo <= 0xDFFF && hi >= 0xD800 {
                if hi > 0xDFFF {
                    stack.push((0xE000, hi));
                }
                if lo >= 0xD800 {
                    break 'split;
                }
                hi = 0xD7FF;
                continue 'split;
            }
            for max in [0x7F, 0x7FF, 0xFFFF] {
                if lo <= max && max < hi {
                    stack.push((max + 1, hi));
                    hi = max;
                    continue 'split;
                }
            }
            if hi <= 0x7F {
                out.push(vec![(lo as u8, hi as u8)]);
                break 'split;
            }
            for shift in 1..4 {
                let mask: u32 = (1 << (6 * shift)) - 1;
                if lo & !mask != hi & !mask {
                    if lo & mask != 0 {
                        stack.push(((lo | mask) + 1, hi));
                        hi = lo | mask;
                        continue 'split;
                    }
                    if hi & mask != mask {
                        stack.push((hi & !mask, hi));
                        hi = (hi & !mask) - 1;
                        continue 'split;
                    }
                }
            }
            let mut start = [0u8; 4];
            let mut end = [0u8; 4];
            let length = encode(lo, &mut start);
            encode(hi, &mut end);
            out.push((0..length).map(|index| (start[index], end[index])).collect());
            break 'split;
        }
    }
}

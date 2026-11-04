trait View {
    fn len(&self) -> usize;
    fn at(&self, index: usize) -> u8;
}

struct Forward<'a>(&'a [u8]);

struct Backward<'a>(&'a [u8]);

struct FoldForward<'a>(&'a [u8]);

struct FoldBackward<'a>(&'a [u8]);

impl View for Forward<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn at(&self, index: usize) -> u8 {
        self.0[index]
    }
}

impl View for Backward<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn at(&self, index: usize) -> u8 {
        self.0[self.0.len() - 1 - index]
    }
}

impl View for FoldForward<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn at(&self, index: usize) -> u8 {
        self.0[index].to_ascii_lowercase()
    }
}

impl View for FoldBackward<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn at(&self, index: usize) -> u8 {
        self.0[self.0.len() - 1 - index].to_ascii_lowercase()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TwoWay {
    critical: usize,
    period: usize,
    periodic: bool,
}

fn maximal_suffix<N: View>(needle: &N, greater: bool) -> (usize, usize) {
    let mut left = 0;
    let mut right = 1;
    let mut offset = 0;
    let mut period = 1;
    while right + offset < needle.len() {
        let candidate = needle.at(right + offset);
        let current = needle.at(left + offset);
        if (candidate < current && !greater) || (candidate > current && greater) {
            right += offset + 1;
            offset = 0;
            period = right - left;
        } else if candidate == current {
            if offset + 1 == period {
                right += offset + 1;
                offset = 0;
            } else {
                offset += 1;
            }
        } else {
            left = right;
            right += 1;
            offset = 0;
            period = 1;
        }
    }
    (left, period)
}

impl TwoWay {
    fn build<N: View>(needle: &N) -> Self {
        let (less, less_period) = maximal_suffix(needle, false);
        let (greater, greater_period) = maximal_suffix(needle, true);
        let (critical, period) = if less > greater {
            (less, less_period)
        } else {
            (greater, greater_period)
        };
        let length = needle.len();
        if critical + period <= length
            && (0..critical).all(|index| needle.at(index) == needle.at(index + period))
        {
            Self {
                critical,
                period,
                periodic: true,
            }
        } else {
            Self {
                critical,
                period: critical.max(length - critical) + 1,
                periodic: false,
            }
        }
    }

    pub fn forward(needle: &[u8]) -> Self {
        Self::build(&Forward(needle))
    }

    pub fn backward(needle: &[u8]) -> Self {
        Self::build(&Backward(needle))
    }

    fn search<N: View, H: View>(&self, needle: &N, haystack: &H, mut at: usize) -> Option<usize> {
        let length = needle.len();
        let mut memory = 0;
        while at + length <= haystack.len() {
            let mut index = self.critical.max(memory);
            while index < length && needle.at(index) == haystack.at(at + index) {
                index += 1;
            }
            if index < length {
                at += index - self.critical + 1;
                memory = 0;
                continue;
            }
            let mut back = self.critical;
            while back > memory && needle.at(back - 1) == haystack.at(at + back - 1) {
                back -= 1;
            }
            if back <= memory {
                return Some(at);
            }
            at += self.period;
            if self.periodic {
                memory = length - self.period;
            }
        }
        None
    }

    pub fn find(&self, needle: &[u8], haystack: &[u8], from: usize, fold: bool) -> Option<usize> {
        if fold {
            self.search(&Forward(needle), &FoldForward(haystack), from)
        } else {
            self.search(&Forward(needle), &Forward(haystack), from)
        }
    }

    pub fn rfind(&self, needle: &[u8], haystack: &[u8], end: usize, fold: bool) -> Option<usize> {
        let haystack = &haystack[..end];
        let found = if fold {
            self.search(&Backward(needle), &FoldBackward(haystack), 0)
        } else {
            self.search(&Backward(needle), &Backward(haystack), 0)
        };
        found.map(|at| haystack.len() - at - needle.len())
    }
}

use std::cmp::{self, Ordering};

use azalea_core::math::{EPSILON, gcd, lcm};

use super::CubePointRange;

#[derive(Debug)]
pub enum IndexMerger {
    Identical {
        coords: Box<[f64]>,
    },
    DiscreteCube {
        result: CubePointRange,
        first_div: u32,
        second_div: u32,
    },
    NonOverlapping {
        lower: Box<[f64]>,
        upper: Box<[f64]>,
        swap: bool,
    },
    Indirect {
        result: Box<[f64]>,
        first_indices: Box<[isize]>,
        second_indices: Box<[isize]>,
        result_length: usize,
    },
}

impl IndexMerger {
    pub fn get_list(&self) -> Box<[f64]> {
        match self {
            IndexMerger::Identical { coords } => coords.clone(),
            IndexMerger::DiscreteCube { result, .. } => result.iter(),
            IndexMerger::NonOverlapping { lower, upper, .. } => (0..self.size())
                .map(|i| {
                    if i < lower.len() {
                        // network-safe: i < lower.len() checked
                        lower[i]
                    } else {
                        // network-safe: i < size() = lower.len() + upper.len()
                        upper[i - lower.len()]
                    }
                })
                .collect(),
            IndexMerger::Indirect {
                result,
                result_length,
                ..
            } => {
                if *result_length <= 1 {
                    Box::new([])
                } else {
                    // network-safe: result_length <= result.len(): new_indirect
                    // adds at most one entry per index step
                    result[..*result_length].into()
                }
            }
        }
    }
    pub fn for_merged_indexes(&self, mut consumer: impl IndexConsumer) -> bool {
        match self {
            IndexMerger::Identical { coords } => {
                for coord in 0..(coords.len() - 1) {
                    if !consumer(coord as i32, coord as i32, coord as i32) {
                        return false;
                    }
                }
                true
            }
            IndexMerger::DiscreteCube {
                result,
                first_div,
                second_div,
            } => {
                for var3 in 0..(result.size() - 1) {
                    // network-safe: DiscreteCube is never built
                    // (new_discrete_cube's only caller is commented out in
                    // create_index_merger)
                    if !consumer(
                        (var3 / second_div).try_into().unwrap(),
                        // network-safe: the same
                        (var3 / first_div).try_into().unwrap(),
                        // network-safe: the same
                        var3.try_into().unwrap(),
                    ) {
                        return false;
                    }
                }
                true
            }
            IndexMerger::NonOverlapping { lower, upper, swap } => {
                if *swap {
                    for_non_swapped_indexes(lower, upper, move |var1x, var2, var3| {
                        consumer(var2, var1x, var3)
                    })
                } else {
                    for_non_swapped_indexes(lower, upper, consumer)
                }
            }
            IndexMerger::Indirect {
                first_indices,
                second_indices,
                result_length,
                ..
            } => {
                let var2 = result_length - 1;

                for var3 in 0..var2 {
                    // network-safe: indexes are -1..len of their coords, var3 <
                    // result_length <= the arrays' length
                    if !consumer(
                        first_indices[var3].try_into().unwrap(),
                        // network-safe: the same
                        second_indices[var3].try_into().unwrap(),
                        // network-safe: the same
                        var3.try_into().unwrap(),
                    ) {
                        return false;
                    }
                }

                true
            }
        }
    }
    pub fn size(&self) -> usize {
        match self {
            IndexMerger::Identical { coords } => coords.len(),
            // network-safe: DiscreteCube is never built (see
            // for_merged_indexes)
            IndexMerger::DiscreteCube { result, .. } => result.size().try_into().unwrap(),
            IndexMerger::NonOverlapping { lower, upper, .. } => lower.len() + upper.len(),
            IndexMerger::Indirect { result_length, .. } => *result_length,
        }
    }

    pub fn new_discrete_cube(a: u32, b: u32) -> Self {
        let result = CubePointRange {
            // network-safe: never called (see for_merged_indexes); static shape
            // resolutions if it were
            parts: (u32::try_from(lcm(a, b)).expect("lcm should be able to fit in a u32"))
                .try_into()
                .expect("lcm should not be 0"),
        };
        let gcd = gcd(a, b);
        // network-safe: the same
        let first_div = a / gcd;
        // network-safe: the same
        let second_div = b / gcd;
        Self::DiscreteCube {
            result,
            first_div,
            second_div,
        }
    }

    pub fn new_indirect(coords1: &[f64], coords2: &[f64], var3: bool, var4: bool) -> Self {
        let mut var5 = f64::NAN;
        let coords1_len = coords1.len();
        let coords2_len = coords2.len();
        let number_of_indices = coords1_len + coords2_len;
        let mut result = Box::<[f64]>::from(vec![0.; number_of_indices]);
        let mut first_indices = Box::<[isize]>::from(vec![0; number_of_indices]);
        let mut second_indices = Box::<[isize]>::from(vec![0; number_of_indices]);
        let var10 = !var3;
        let var11 = !var4;
        let mut var12 = 0;
        let mut coords1_index = 0;
        let mut coords2_index = 0;

        loop {
            let mut iterating_coords1: bool;
            loop {
                let at_end_of_coords1 = coords1_index >= coords1_len;
                let at_end_of_coords2 = coords2_index >= coords2_len;
                if at_end_of_coords1 && at_end_of_coords2 {
                    let result_length = cmp::max(1, var12);
                    return Self::Indirect {
                        result,
                        first_indices,
                        second_indices,
                        result_length,
                    };
                }

                // network-safe: coords1_index < coords1_len (not at its end),
                // and coords2_index only read when not at its end
                iterating_coords1 = !at_end_of_coords1
                    && (at_end_of_coords2
                        || coords1[coords1_index] < coords2[coords2_index] + EPSILON);
                if iterating_coords1 {
                    coords1_index += 1;
                    if !var10 || coords2_index != 0 && !at_end_of_coords2 {
                        break;
                    }
                } else {
                    coords2_index += 1;
                    if !var11 || coords1_index != 0 && !at_end_of_coords1 {
                        break;
                    }
                }
            }

            let var18: isize = (coords1_index as isize) - 1;
            let var19: isize = (coords2_index as isize) - 1;
            let var20 = if iterating_coords1 {
                // network-safe: coords1_index was just incremented, so var18 >=
                // 0 and < coords1_len
                coords1[usize::try_from(var18).unwrap()]
            } else {
                // network-safe: coords2_index was just incremented, so var19 >=
                // 0 and < coords2_len
                coords2[usize::try_from(var19).unwrap()]
            };
            match var5.partial_cmp(&(var20 - EPSILON)) {
                None | Some(Ordering::Less) => {
                    // network-safe: var12 < coords1_len + coords2_len: at most
                    // one entry per index step
                    result[var12] = var20;
                    // network-safe: the same
                    first_indices[var12] = var18;
                    // network-safe: the same
                    second_indices[var12] = var19;
                    var12 += 1;
                    var5 = var20;
                }
                _ => {
                    // network-safe: var12 >= 1: the first step takes the first
                    // branch (var5 starts NaN)
                    first_indices[var12 - 1] = var18;
                    // network-safe: the same
                    second_indices[var12 - 1] = var19;
                }
            }
        }
    }
}

pub trait IndexConsumer = FnMut(i32, i32, i32) -> bool;

fn for_non_swapped_indexes(lower: &[f64], upper: &[f64], mut consumer: impl IndexConsumer) -> bool {
    let var2 = lower.len();
    for var3 in 0..var2 {
        // network-safe: var3 < lower.len(), a small shape size
        if !consumer(var3.try_into().unwrap(), -1, var3.try_into().unwrap()) {
            return false;
        }
    }
    let var3 = upper.len() - 1;
    for var4 in 0..var3 {
        // network-safe: small shape sizes
        if !consumer(
            (var2 - 1).try_into().unwrap(),
            // network-safe: the same
            var4.try_into().unwrap(),
            // network-safe: the same
            (var2 + var4).try_into().unwrap(),
        ) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_indirect_index_merger() {
        IndexMerger::new_indirect(&[0.0, 1.0], &[0.0, 0.5, 1.0], true, true);
    }
}

//! Linear circuits of resistors and capacitors, solved by nodal analysis
//! with every capacitor discretised by the trapezoidal rule (the bilinear
//! transform, as every other linear stage in the family), for the models
//! whose circuit is more than a chain of filters: where paths meet at a
//! summing node, or a nonlinear part sits inside a network that loads it.
//!
//! A capacitor `C` becomes, each step, a conductance `g = 2 C fs` beside a
//! current source carrying its history (`i = g v - h`, then
//! `h' = 2 g v - h`). The nodal matrix, conductances between the nodes,
//! depends only on the parts and the rate, so it is inverted once per
//! design and each step is one matrix-vector product. A nonlinear part at
//! one node is then solved against the network's exact Thévenin
//! equivalent there: the node's voltage with nothing drawn from it
//! ([`Network::open`]) behind the impedance [`Network::impedance`] of the
//! node to itself.

/// Where one end of a part connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// A node the network solves for, by index.
    Node(usize),
    /// Ground (for a pedal, its bias rail: an AC ground).
    Ground,
    /// The virtual ground of an inverting summing amplifier: ground to the
    /// network, but the current into it is the network's output
    /// ([`Network::sum_current`]).
    Sum,
    /// A voltage the network is driven by, by index.
    Source(usize),
}

/// What a part is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// A resistor, in ohms.
    Resistor(f64),
    /// A capacitor, in farads.
    Capacitor(f64),
}

/// A two-terminal part between `a` and `b`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Part {
    pub a: End,
    pub b: End,
    pub kind: Kind,
}

impl Part {
    /// A resistor of `ohms` between `a` and `b`.
    pub const fn resistor(a: End, b: End, ohms: f64) -> Self {
        Self {
            a,
            b,
            kind: Kind::Resistor(ohms),
        }
    }

    /// A capacitor of `farads` between `a` and `b`.
    pub const fn capacitor(a: End, b: End, farads: f64) -> Self {
        Self {
            a,
            b,
            kind: Kind::Capacitor(farads),
        }
    }
}

/// Below this a capacitor's history is taken as zero, so a decaying
/// network settles to exact silence rather than into subnormals.
const QUIET: f64 = 1e-30;

/// A network of `P` parts over `N` nodes driven by `S` sources.
#[derive(Debug, Clone, Copy)]
pub struct Network<const N: usize, const P: usize, const S: usize> {
    parts: [Part; P],
    /// Each part's conductance at the designed rate.
    conductance: [f64; P],
    /// The inverse of the nodal matrix: the voltage at each node for a
    /// unit current into each node.
    impedance: [[f64; N]; N],
    /// How much current into [`End::Sum`] each node's voltage drives.
    to_sum: [f64; N],
    /// Each capacitor's history current (zero for a resistor), flowing
    /// from its `a` end to its `b` end.
    history: [f64; P],
}

impl<const N: usize, const P: usize, const S: usize> Network<N, P, S> {
    /// A network of `parts`, designed for the rate whose bilinear constant
    /// (twice the rate) is `c`.
    pub fn new(parts: [Part; P], c: f64) -> Self {
        let mut network = Self {
            parts,
            conductance: [0.0; P],
            impedance: [[0.0; N]; N],
            to_sum: [0.0; N],
            history: [0.0; P],
        };
        network.design(c);
        network
    }

    /// Set resistor `index` to `ohms` (a pot turned); takes effect at the
    /// next [`Self::design`]. Other parts are left as they are.
    pub fn set_resistance(&mut self, index: usize, ohms: f64) {
        if let Some(part) = self.parts.get_mut(index)
            && let Kind::Resistor(value) = &mut part.kind
        {
            *value = ohms;
        }
    }

    /// Work out the conductances and invert the nodal matrix for the rate
    /// whose bilinear constant is `c`. Capacitor histories carry over, so
    /// a pot can move mid-note.
    pub fn design(&mut self, c: f64) {
        let mut matrix = [[0.0; N]; N];
        self.to_sum = [0.0; N];
        for (part, conductance) in self.parts.iter().zip(&mut self.conductance) {
            let g = match part.kind {
                Kind::Resistor(ohms) => 1.0 / ohms,
                Kind::Capacitor(farads) => farads * c,
            };
            *conductance = g;
            for (near, far) in [(part.a, part.b), (part.b, part.a)] {
                if let End::Node(i) = near {
                    matrix[i][i] += g;
                    match far {
                        End::Node(j) => matrix[i][j] -= g,
                        End::Sum => self.to_sum[i] += g,
                        End::Ground | End::Source(_) => {}
                    }
                }
            }
        }
        self.impedance = inverse(matrix);
    }

    /// Forget every capacitor's charge.
    pub const fn reset(&mut self) {
        self.history = [0.0; P];
    }

    /// The impedance between nodes `i` and `j`: the voltage at `i` for a
    /// unit current drawn out of `j` is minus this.
    pub const fn impedance(&self, i: usize, j: usize) -> f64 {
        self.impedance[i][j]
    }

    /// How much current into the summing node a unit current drawn out of
    /// node `j` takes away.
    pub fn sum_pull(&self, j: usize) -> f64 {
        self.to_sum
            .iter()
            .zip(&self.impedance)
            .map(|(weight, row)| weight * row[j])
            .sum()
    }

    /// Every node's voltage this step with `sources` driving and nothing
    /// drawn from any node.
    pub fn open(&self, sources: [f64; S]) -> [f64; N] {
        let mut injected = [0.0; N];
        for ((part, g), h) in self.parts.iter().zip(&self.conductance).zip(&self.history) {
            for (near, far, sign) in [(part.a, part.b, 1.0), (part.b, part.a, -1.0)] {
                if let End::Node(i) = near {
                    injected[i] += sign * h;
                    if let End::Source(k) = far {
                        injected[i] += g * sources[k];
                    }
                }
            }
        }
        let mut volts = [0.0; N];
        for (volt, row) in volts.iter_mut().zip(&self.impedance) {
            *volt = row.iter().zip(&injected).map(|(z, i)| z * i).sum();
        }
        volts
    }

    /// The current into [`End::Sum`] with the nodes at `volts` and
    /// `sources` driving, before [`Self::settle`] moves the step on.
    pub fn sum_current(&self, volts: &[f64; N], sources: [f64; S]) -> f64 {
        let mut total = 0.0;
        for ((part, g), h) in self.parts.iter().zip(&self.conductance).zip(&self.history) {
            let from = |end: End| match end {
                End::Node(i) => volts[i],
                End::Source(k) => sources[k],
                End::Ground | End::Sum => 0.0,
            };
            let through = g * (from(part.a) - from(part.b)) - h;
            if part.b == End::Sum {
                total += through;
            } else if part.a == End::Sum {
                total -= through;
            }
        }
        total
    }

    /// End the step with the nodes at `volts` and `sources` driving: each
    /// capacitor's history moves on by the trapezoidal rule.
    pub fn settle(&mut self, volts: &[f64; N], sources: [f64; S]) {
        for ((part, g), h) in self
            .parts
            .iter()
            .zip(&self.conductance)
            .zip(&mut self.history)
        {
            if let Kind::Capacitor(_) = part.kind {
                let from = |end: End| match end {
                    End::Node(i) => volts[i],
                    End::Source(k) => sources[k],
                    End::Ground | End::Sum => 0.0,
                };
                let next = (2.0 * g).mul_add(from(part.a) - from(part.b), -*h);
                *h = if next.is_finite() && next.abs() >= QUIET {
                    next
                } else {
                    0.0
                };
            }
        }
    }
}

/// The inverse of a nodal matrix by Gauss-Jordan elimination. A nodal
/// matrix of positive conductances in which every node reaches ground or a
/// source is symmetric and positive definite, so elimination needs no
/// pivoting and every pivot is positive.
fn inverse<const N: usize>(mut matrix: [[f64; N]; N]) -> [[f64; N]; N] {
    let mut inverse = [[0.0; N]; N];
    for (i, row) in inverse.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for col in 0..N {
        let pivot = matrix[col][col];
        for k in 0..N {
            matrix[col][k] /= pivot;
            inverse[col][k] /= pivot;
        }
        for row in 0..N {
            if row != col {
                let factor = matrix[row][col];
                for k in 0..N {
                    matrix[row][k] = factor.mul_add(-matrix[col][k], matrix[row][k]);
                    inverse[row][k] = factor.mul_add(-inverse[col][k], inverse[row][k]);
                }
            }
        }
    }
    inverse
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: f64 = 2.0 * 48_000.0;

    /// A divider of 1 kΩ over 3 kΩ from a source: three quarters of it.
    #[test]
    fn a_divider_divides() {
        let network: Network<1, 2, 1> = Network::new(
            [
                Part::resistor(End::Source(0), End::Node(0), 1e3),
                Part::resistor(End::Node(0), End::Ground, 3e3),
            ],
            C,
        );
        let volts = network.open([2.0]);
        assert!((volts[0] - 1.5).abs() < 1e-12, "{}", volts[0]);
        assert!((network.impedance(0, 0) - 750.0).abs() < 1e-9);
    }

    /// An RC lowpass stepped by the network matches the bilinear transform
    /// of `1 / (1 + s R C)`, sample for sample.
    #[test]
    fn an_rc_lowpass_is_the_bilinear_one() {
        let (r, cap) = (10e3, 100e-9);
        let mut network: Network<1, 2, 1> = Network::new(
            [
                Part::resistor(End::Source(0), End::Node(0), r),
                Part::capacitor(End::Node(0), End::Ground, cap),
            ],
            C,
        );
        // y = (x + x') / (1 + k) - y' (1 - k) / (1 + k), k = c R C.
        let k = C * r * cap;
        let (mut x1, mut y1) = (0.0, 0.0);
        for n in 0..2_000 {
            let x = (f64::from(n) * 0.05).sin();
            let volts = network.open([x]);
            network.settle(&volts, [x]);
            let y = (x + x1) / (1.0 + k) - y1 * (1.0 - k) / (1.0 + k);
            assert!((volts[0] - y).abs() < 1e-12, "step {n}: {} {y}", volts[0]);
            (x1, y1) = (x, y);
        }
    }

    /// The current into a summing node through a resistor and through a
    /// capacitor, with the Thévenin pull of a load drawn at the far node.
    #[test]
    fn the_summing_node_takes_every_path() {
        let network: Network<1, 3, 1> = Network::new(
            [
                Part::resistor(End::Source(0), End::Node(0), 1e3),
                Part::resistor(End::Node(0), End::Sum, 1e3),
                Part::resistor(End::Source(0), End::Sum, 2e3),
            ],
            C,
        );
        let volts = network.open([1.0]);
        // Node 0 is half the source; 0.5 mA through the lower resistor and
        // 0.5 mA straight across.
        let current = network.sum_current(&volts, [1.0]);
        assert!((current - 1e-3).abs() < 1e-15, "{current}");
        // Drawing 1 mA out of node 0 drops it by 0.5 V: 0.5 mA less in.
        assert!((network.sum_pull(0) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn a_pot_turned_is_taken_at_the_next_design() {
        let mut network: Network<1, 2, 1> = Network::new(
            [
                Part::resistor(End::Source(0), End::Node(0), 1e3),
                Part::resistor(End::Node(0), End::Ground, 1e3),
            ],
            C,
        );
        network.set_resistance(1, 3e3);
        assert!((network.open([1.0])[0] - 0.5).abs() < 1e-12);
        network.design(C);
        assert!((network.open([1.0])[0] - 0.75).abs() < 1e-12);
    }

    #[test]
    fn a_decayed_network_is_exactly_silent() {
        let mut network: Network<1, 2, 1> = Network::new(
            [
                Part::resistor(End::Source(0), End::Node(0), 1e3),
                Part::capacitor(End::Node(0), End::Ground, 1e-6),
            ],
            C,
        );
        let volts = network.open([1.0]);
        network.settle(&volts, [1.0]);
        for _ in 0..200_000 {
            let volts = network.open([0.0]);
            network.settle(&volts, [0.0]);
        }
        assert_eq!(network.open([0.0])[0].to_bits(), 0.0f64.to_bits());
    }
}

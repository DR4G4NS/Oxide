// Documented non-convergence witness pattern for TOOL001.
// The analyzer's SCC engine must fail closed when identity substitution
// compounds without reaching a finite fixed point. See callgraph::tests.

struct R;

impl R {
    fn a(&self) {
        self.b();
    }
    fn b(&self) {
        self.a();
    }
}

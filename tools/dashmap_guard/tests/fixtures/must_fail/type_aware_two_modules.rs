mod alpha {
    use dashmap::DashMap;

    pub struct Holder {
        pub alpha_map: DashMap<u32, u32>,
    }

    impl Holder {
        pub fn touch(&self) {
            self.alpha_map.insert(1, 1);
        }
    }
}

mod beta {
    use dashmap::DashMap;

    pub struct Holder {
        pub beta_map: DashMap<u32, u32>,
    }

    impl Holder {
        pub fn touch(&self) {
            self.beta_map.insert(2, 2);
        }
    }
}

fn use_alpha(h: &alpha::Holder) {
    h.touch();
}

fn use_beta(h: &beta::Holder) {
    h.touch();
}

fn bug_alpha(h: &alpha::Holder) {
    let guard = h.alpha_map.get(&1).unwrap();
    use_alpha(h);
    let _ = guard;
}

fn bug_beta(h: &beta::Holder) {
    let guard = h.beta_map.get(&2).unwrap();
    use_beta(h);
    let _ = guard;
}

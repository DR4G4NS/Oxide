//! Logic compiler: source text -> Program (LAssembler + LParser subset).

use super::executor::{
    ApplyStatusSpec, DrawSpec, DrawType, ExplosionSpec, Instr, LObject, LVar, LogicRule, RadarSort,
    RadarSpec, RadarTarget, SetPropKey, SetPropSpec, SetRuleSpec, SpawnSpec, UcOp, UlocGroup,
    UlocKind, UlocSpec,
};
use super::ops::{
    item_id_from_name, liquid_id_from_name, team_id_from_token, try_item_id_from_name, Cond,
    FetchKind, LAccess, LookupKind, Op, TileLayer,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Expression AST for `set`/`print` and value operands.
#[derive(Clone, Debug)]
pub enum Expr {
    Num(f64),
    Str(String),
    /// Compile-time UnitType content (`@dagger` etc.).
    UnitType(i16),
    /// Compile-time Team content (`@crux` etc.).
    Team(u8),
    Var(usize),
    Bin(Op, Box<Expr>, Box<Expr>),
}

/// A compiled program: variable table + instructions.
pub struct Program {
    pub vars: Vec<LVar>,
    pub instructions: Vec<Instr>,
    /// Index of the @counter variable.
    #[allow(dead_code)]
    pub counter_var: usize,
    /// Index of the @this variable (runtime-filled per tile).
    pub this_var: usize,
    /// Index of the @unit variable (always null in phase 1).
    #[allow(dead_code)]
    pub unit_var: usize,
    /// Index of the @links variable (runtime-filled).
    pub links_var: usize,
    /// Index of the @ipt variable (runtime-filled).
    pub ipt_var: usize,
}

impl Program {
    #[allow(dead_code)]
    pub fn var_index(&self, name: &str) -> Option<usize> {
        self.vars.iter().position(|v| v.name == name)
    }
}

/// Compiler: source text -> Program (LAssembler + LParser subset).
pub fn compile(source: &str) -> Option<Arc<Program>> {
    compile_report(source).0
}

/// P0-7: compiles `source` and returns the program together with every
/// structured diagnostic (unsupported statements and malformed arity with
/// source line numbers). Strict callers reject programs with diagnostics
/// instead of silently running a degraded program.
pub fn compile_report(source: &str) -> (Option<Arc<Program>>, Vec<String>) {
    let mut asm = Assembler::new();
    if asm.compile(source).is_none() {
        return (None, asm.diagnostics);
    }
    let program = match (
        asm.counter_var,
        asm.this_var,
        asm.unit_var,
        asm.links_var,
        asm.ipt_var,
    ) {
        (Some(counter_var), Some(this_var), Some(unit_var), Some(links_var), Some(ipt_var)) => {
            Some(Arc::new(Program {
                vars: asm.vars,
                instructions: asm.instructions,
                counter_var,
                this_var,
                unit_var,
                links_var,
                ipt_var,
            }))
        }
        _ => None,
    };
    (program, asm.diagnostics)
}

pub struct Assembler {
    vars: Vec<LVar>,
    name_map: HashMap<String, usize>,
    instructions: Vec<Instr>,
    /// Named labels -> instruction index.
    labels: HashMap<String, i32>,
    counter_var: Option<usize>,
    this_var: Option<usize>,
    unit_var: Option<usize>,
    links_var: Option<usize>,
    ipt_var: Option<usize>,
    warned: std::collections::HashSet<String>,
    /// P0-7: structured diagnostics for statements that cannot be compiled
    /// faithfully (unsupported names or malformed arity). Populated in
    /// strict mode so callers can reject silently-degraded programs.
    pub(crate) diagnostics: Vec<String>,
}

impl Assembler {
    pub(crate) fn new() -> Assembler {
        let mut asm = Assembler {
            vars: Vec::new(),
            name_map: HashMap::new(),
            instructions: Vec::new(),
            labels: HashMap::new(),
            counter_var: None,
            this_var: None,
            unit_var: None,
            links_var: None,
            ipt_var: None,
            warned: std::collections::HashSet::new(),
            diagnostics: Vec::new(),
        };
        // Standard constants (LAssembler constructor).
        asm.counter_var = Some(asm.put_var("@counter", false));
        asm.this_var = Some(asm.put_const_obj("@this", LObject::Null));
        asm.unit_var = Some(asm.put_const_obj("@unit", LObject::Null));
        asm.links_var = Some(asm.put_const_num("@links", 0.0));
        asm.ipt_var = Some(asm.put_const_num("@ipt", 8.0));
        asm
    }

    fn put_var(&mut self, name: &str, constant: bool) -> usize {
        if let Some(idx) = self.name_map.get(name) {
            return *idx;
        }
        let idx = self.vars.len();
        let mut v = LVar::new_var(name);
        v.constant = constant;
        self.vars.push(v);
        self.name_map.insert(name.to_string(), idx);
        idx
    }

    fn put_const_num(&mut self, name: &str, value: f64) -> usize {
        if let Some(idx) = self.name_map.get(name) {
            return *idx;
        }
        let idx = self.vars.len();
        self.vars.push(LVar::new_num(name, value));
        self.name_map.insert(name.to_string(), idx);
        idx
    }

    fn put_const_obj(&mut self, name: &str, value: LObject) -> usize {
        if let Some(idx) = self.name_map.get(name) {
            return *idx;
        }
        let idx = self.vars.len();
        self.vars.push(LVar::new_obj(name, value));
        self.name_map.insert(name.to_string(), idx);
        idx
    }

    /// `asm.var(symbol)`: constants, strings, numbers or named variables.
    fn var(&mut self, symbol: &str) -> usize {
        let symbol = symbol.trim();
        // @counter etc are already registered by name.
        if let Some(idx) = self.name_map.get(symbol) {
            return *idx;
        }
        // string case
        if symbol.len() >= 2 && symbol.starts_with('"') && symbol.ends_with('"') {
            let value = symbol[1..symbol.len() - 1].replace("\\n", "\n");
            let key = format!("___{value}");
            if let Some(idx) = self.name_map.get(&key) {
                return *idx;
            }
            let idx = self.vars.len();
            self.vars.push(LVar::new_obj(&key, LObject::Str(value)));
            self.name_map.insert(key, idx);
            return idx;
        }
        let cleaned = symbol.replace(' ', "_");
        // GlobalVars registers Arc Align values used by `draw print` as
        // constants. Keep these aliases numeric so ordinary source such as
        // `draw print 0 0 @topLeft` does not allocate a writable variable.
        let align = match cleaned.as_str() {
            "@center" | "center" => Some(1.0),
            "@top" | "top" => Some(2.0),
            "@bottom" | "bottom" => Some(4.0),
            "@left" | "left" => Some(8.0),
            "@right" | "right" => Some(16.0),
            "@topLeft" | "topLeft" => Some(10.0),
            "@topRight" | "topRight" => Some(18.0),
            "@bottomLeft" | "bottomLeft" => Some(12.0),
            "@bottomRight" | "bottomRight" => Some(20.0),
            _ => None,
        };
        if let Some(value) = align.or_else(|| parse_number(&cleaned)) {
            return self.put_const_num(&format!("___{cleaned}"), value);
        }
        self.put_var(&cleaned, false)
    }

    /// Team operand for `fetch`/`setblock`: official team content constants
    /// (`@sharded` -> 1, Team.all ordinal) resolve to const numbers; any
    /// other token is a runtime expression (numeric team ids are valid).
    fn team_expr(&mut self, token: &str) -> Expr {
        if let Some(team) = team_id_from_token(token) {
            return Expr::Num(team);
        }
        self.expr(&[token.to_string()])
    }

    /// Resolve a literal content/team token to its numeric id when known.
    fn content_literal_num(&self, token: &str) -> Option<f64> {
        let name = token.trim().trim_start_matches('@').trim_matches('"');
        if name.is_empty() {
            return None;
        }
        crate::network::units::parse_unit_type(name)
            .map(f64::from)
            .or_else(|| crate::game::block_names::block_id_from_name(name).map(f64::from))
            .or_else(|| crate::logic::ops::team_id_from_token(token))
    }

    /// Content operand for the `setblock` block field: literal content
    /// tokens (`@conveyor` or `conveyor`) resolve to their official content
    /// id as a const number; anything else is a runtime expression.
    fn content_expr(&mut self, token: &str) -> Expr {
        if let Some(value) = self.content_literal_num(token) {
            return Expr::Num(value);
        }
        self.expr(&[token.to_string()])
    }

    /// Spawn type operand: resolves literal unit names to UnitType objects.
    fn unit_type_expr(&mut self, token: &str) -> Expr {
        let token = token.trim();
        // Only registered @name content constants are UnitType objects.
        // `parse_unit_type` also accepts numeric console IDs, which must not
        // be used by SpawnUnitI's LAssembler path. Bare names, @numeric, and
        // quoted names remain variables/numbers/strings and fail the runtime
        // `type.obj() instanceof UnitType` gate as Java does.
        if let Some(name) = token.strip_prefix('@') {
            if !name.is_empty() && name.parse::<i16>().is_err() {
                if let Some(id) = crate::game::unit_types::UNIT_NAMES
                    .iter()
                    .find(|(_, registered)| *registered == name)
                    .map(|(id, _)| *id)
                {
                    return Expr::UnitType(id);
                }
            }
        }
        if parse_number(token).is_some() || (token.starts_with('"') && token.ends_with('"')) {
            return self.expr(&[token.to_string()]);
        }
        // Keep variable operands as variables even when their name happens
        // to match a unit content name (e.g. `set dagger @dagger`).
        Expr::Var(self.var(token))
    }

    fn compile(&mut self, source: &str) -> Option<()> {
        // Pass 1: collect labels.
        for (line_index, raw) in source.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let tokens = tokenize(line);
            if tokens.is_empty() {
                continue;
            }
            if tokens[0] == "label" && tokens.len() >= 2 {
                let idx = self.instructions.len() as i32;
                self.labels.insert(tokens[1].clone(), idx);
            }
            // Reserve an instruction slot per statement so labels align with
            // the final instruction list.
            self.instructions.push(Instr::NoOp);
            let _ = line_index;
        }
        // Pass 2: rebuild with real instructions.
        let mut built: Vec<Instr> = Vec::new();
        for (line_index, raw) in source.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let tokens = tokenize(line);
            if tokens.is_empty() {
                continue;
            }
            let statement_name = tokens[0].clone();
            let instr = self.build_statement(&tokens);
            // P0-7: a statement that compiled to NoOp without being a
            // structural no-op (label/end) is silent degradation. Record a
            // located diagnostic so strict callers can reject it.
            if matches!(instr, Instr::NoOp)
                && statement_name != "label"
                && statement_name != "end"
                && !self
                    .diagnostics
                    .iter()
                    .any(|d| d.contains(&format!("'{statement_name}'")))
            {
                self.diagnostics.push(format!(
                    "statement '{statement_name}' at line {} compiled to NoOp",
                    line_index + 1
                ));
            }
            built.push(instr);
        }
        self.instructions = built;
        Some(())
    }

    /// Resolves a jump target token: numeric instruction index or named label.
    fn resolve_label(&self, token: &str) -> i32 {
        if let Some(index) = parse_i32(token) {
            return index.clamp(0, self.instructions.len() as i32 - 1);
        }
        self.labels
            .get(token)
            .copied()
            .unwrap_or(0)
            .clamp(0, self.instructions.len() as i32 - 1)
    }

    fn build_statement(&mut self, tokens: &[String]) -> Instr {
        let name = tokens[0].as_str();
        let args = &tokens[1..];
        match name {
            "set" => {
                if args.len() >= 2 {
                    let dest = self.var(&args[0]);
                    let expr = self.expr(&args[1..]);
                    return Instr::Set(dest, expr);
                }
                Instr::NoOp
            }
            "op" => {
                if args.len() >= 3 {
                    if let Some(op) = parse_op(&args[0]) {
                        let dest = self.var(&args[1]);
                        let a = self.expr(&args[2..]);
                        if op.unary() && args.len() >= 3 {
                            return Instr::Op(dest, op, a, None);
                        }
                        if args.len() >= 4 {
                            let b = self.expr(&args[3..]);
                            return Instr::Op(dest, op, a, Some(b));
                        }
                    }
                }
                Instr::NoOp
            }
            "read" => {
                if args.len() >= 3 {
                    let dest = self.var(&args[0]);
                    let cell = self.expr(&args[1..2]);
                    let addr = self.expr(&args[2..]);
                    return Instr::Read(dest, cell, addr);
                }
                Instr::NoOp
            }
            "write" => {
                if args.len() >= 3 {
                    let value = self.expr(&args[0..1]);
                    let cell = self.expr(&args[1..2]);
                    let addr = self.expr(&args[2..]);
                    return Instr::Write(cell, addr, value);
                }
                Instr::NoOp
            }
            "jump" => {
                if args.len() >= 4 {
                    if let Some(cond) = parse_cond(&args[1]) {
                        let target = self.resolve_label(&args[0]);
                        let a = self.expr(&args[2..3]);
                        let b = self.expr(&args[3..]);
                        return Instr::Jump(target, cond, a, b);
                    }
                    // Expression condition form: jump <label> <a> <op> <b>
                    if let Some(cond) = parse_cond(&args[2]) {
                        let target = self.resolve_label(&args[0]);
                        let a = self.expr(&args[1..2]);
                        let b = self.expr(&args[3..]);
                        return Instr::Jump(target, cond, a, b);
                    }
                }
                // Bare condition form: jump <label> <expr>  (non-zero jumps)
                if args.len() >= 2 {
                    let target = self.resolve_label(&args[0]);
                    let a = self.expr(&args[1..]);
                    let b = Expr::Num(0.0);
                    return Instr::Jump(target, Cond::NotEqual, a, b);
                }
                Instr::NoOp
            }
            "wait" => {
                if !args.is_empty() {
                    let value = self.expr(args);
                    return Instr::Wait(value);
                }
                Instr::NoOp
            }
            "getlink" => {
                if args.len() >= 2 {
                    let dest = self.var(&args[0]);
                    let index = self.expr(&args[1..]);
                    return Instr::GetLink(dest, index);
                }
                Instr::NoOp
            }
            "sensor" => {
                if args.len() >= 3 {
                    if let Some(item) = LAccess::parse(&args[2]) {
                        let dest = self.var(&args[0]);
                        let target = self.expr(&args[1..2]);
                        return Instr::Sensor(dest, target, item);
                    }
                }
                Instr::NoOp
            }
            "print" => {
                let exprs: Vec<Expr> = (0..args.len())
                    .map(|i| self.expr(&args[i..i + 1]))
                    .collect();
                if !exprs.is_empty() {
                    return Instr::Print(exprs);
                }
                Instr::NoOp
            }
            "printchar" => {
                // printchar <value> — appends one character to the print buffer.
                if !args.is_empty() {
                    return Instr::PrintChar(self.expr(&args[0..1]));
                }
                Instr::NoOp
            }
            "draw" => {
                // LStatements.DrawStatement always carries six operand slots
                // (x, y, p1..p4), even when the selected GraphicsType uses
                // fewer. Missing source operands therefore become zero while
                // preserving this statement's single instruction slot.
                if let Some(kind) = args.first().and_then(|token| DrawType::parse(token)) {
                    let zero = || Expr::Num(0.0);
                    let operand = |index: usize, asm: &mut Assembler| {
                        args.get(index + 1)
                            .map(|token| asm.expr(std::slice::from_ref(token)))
                            .unwrap_or_else(zero)
                    };
                    return Instr::Draw(DrawSpec {
                        kind,
                        x: operand(0, self),
                        y: operand(1, self),
                        p1: operand(2, self),
                        p2: operand(3, self),
                        p3: operand(4, self),
                        p4: operand(5, self),
                    });
                }
                if !args.is_empty() {
                    self.warn_unsupported(&format!("draw {}", args[0]));
                } else {
                    self.warn_unsupported("draw");
                }
                Instr::NoOp
            }
            "drawflush" => {
                if !args.is_empty() {
                    return Instr::DrawFlush(self.expr(&args[0..1]));
                }
                Instr::NoOp
            }
            "stop" => {
                // stop — halts the processor at this instruction (official
                // StopI: counter--, yield; ignores any trailing tokens).
                Instr::Stop
            }
            "packcolor" => {
                // packcolor <result> <r> <g> <b> <a>
                if args.len() >= 5 {
                    let result = self.var(&args[0]);
                    let r = self.expr(&args[1..2]);
                    let g = self.expr(&args[2..3]);
                    let b = self.expr(&args[3..4]);
                    let a = self.expr(&args[4..5]);
                    return Instr::PackColor(result, r, g, b, a);
                }
                Instr::NoOp
            }
            "unpackcolor" => {
                // unpackcolor <r> <g> <b> <a> <value>
                if args.len() >= 5 {
                    let r = self.var(&args[0]);
                    let g = self.var(&args[1]);
                    let b = self.var(&args[2]);
                    let a = self.var(&args[3]);
                    let value = self.expr(&args[4..5]);
                    return Instr::UnpackColor(r, g, b, a, value);
                }
                Instr::NoOp
            }
            "printflush" => {
                if !args.is_empty() {
                    let target = self.expr(&args[0..1]);
                    return Instr::PrintFlush(target);
                }
                Instr::NoOp
            }
            "control" => {
                if args.len() >= 2 {
                    let ctrl = args[0].as_str();
                    let target = self.expr(&args[1..2]);
                    match ctrl {
                        "enabled" => {
                            let value = if args.len() >= 3 {
                                self.expr(&args[2..3])
                            } else {
                                Expr::Num(1.0)
                            };
                            return Instr::ControlEnabled(target, value);
                        }
                        _ => {
                            self.warn_unsupported(&format!("control {ctrl}"));
                            return Instr::NoOp;
                        }
                    }
                }
                Instr::NoOp
            }
            "setrate" => {
                if !args.is_empty() {
                    let value = self.expr(args);
                    return Instr::SetRate(value);
                }
                Instr::NoOp
            }
            "ubind" => {
                if !args.is_empty() {
                    let name = args[0].trim_start_matches('@');
                    if let Some(unit_type) = crate::network::units::parse_unit_type(name) {
                        return Instr::Ubind(unit_type);
                    }
                    // Runtime operand (official UnitBindI evaluates
                    // `type.obj()` when the instruction runs): a variable
                    // holding a Unit object — a radar/fetch/ubind result —
                    // binds that exact unit; anything else binds nothing.
                    return Instr::UbindExpr(self.expr(&args[0..1]));
                }
                self.warn_unsupported("ubind");
                Instr::NoOp
            }
            "ucontrol" => {
                if args.is_empty() {
                    return Instr::NoOp;
                }
                let sub = args[0].as_str();
                let rest = &args[1..];
                match sub {
                    "move" if rest.len() >= 2 => {
                        let x = self.expr(&rest[0..1]);
                        let y = self.expr(&rest[1..2]);
                        Instr::Ucontrol(UcOp::Move(x, y))
                    }
                    "stop" => Instr::Ucontrol(UcOp::Stop),
                    "within" if rest.len() >= 4 => {
                        let x = self.expr(&rest[0..1]);
                        let y = self.expr(&rest[1..2]);
                        let radius = self.expr(&rest[2..3]);
                        let dest = self.var(&rest[3]);
                        Instr::Ucontrol(UcOp::Within(x, y, radius, dest))
                    }
                    "getBlock" if rest.len() >= 4 => {
                        let x = self.expr(&rest[0..1]);
                        let y = self.expr(&rest[1..2]);
                        let building = self.var(&rest[2]);
                        let floor = self.var(&rest[3]);
                        Instr::Ucontrol(UcOp::GetBlock(x, y, building, floor))
                    }
                    "flag" if !rest.is_empty() => {
                        Instr::Ucontrol(UcOp::Flag(self.expr(&rest[0..1])))
                    }
                    "boost" if !rest.is_empty() => {
                        Instr::Ucontrol(UcOp::Boost(self.expr(&rest[0..1])))
                    }
                    "mine" if rest.len() >= 2 => {
                        let x = self.expr(&rest[0..1]);
                        let y = self.expr(&rest[1..2]);
                        Instr::Ucontrol(UcOp::Mine(x, y))
                    }
                    "itemDrop" if rest.len() >= 2 => {
                        let to = self.expr(&rest[0..1]);
                        let amount = self.expr(&rest[1..2]);
                        Instr::Ucontrol(UcOp::ItemDrop(to, amount))
                    }
                    "itemTake" if rest.len() >= 3 => {
                        let from = self.expr(&rest[0..1]);
                        let item = item_id_from_name(rest[1].trim_start_matches('@'));
                        let amount = self.expr(&rest[2..3]);
                        Instr::Ucontrol(UcOp::ItemTake(from, item, amount))
                    }
                    "shoot" | "target" if rest.len() >= 3 => {
                        let x = self.expr(&rest[0..1]);
                        let y = self.expr(&rest[1..2]);
                        let shoot = self.expr(&rest[2..3]);
                        if sub == "shoot" {
                            Instr::Ucontrol(UcOp::Shoot(x, y, shoot))
                        } else {
                            Instr::Ucontrol(UcOp::Target(x, y, shoot))
                        }
                    }
                    "build" if rest.len() >= 4 => {
                        let x = self.expr(&rest[0..1]);
                        let y = self.expr(&rest[1..2]);
                        let block = rest[2].trim_start_matches('@').trim_matches('"');
                        if let Some(block_id) = crate::game::block_names::block_id_from_name(block)
                        {
                            let rotation = self.expr(&rest[3..4]);
                            Instr::Ucontrol(UcOp::Build(x, y, block_id, rotation))
                        } else {
                            self.warn_unsupported("ucontrol build (unknown block)");
                            Instr::NoOp
                        }
                    }
                    "unbind" => Instr::Ucontrol(UcOp::Unbind),
                    "pathfind" if rest.len() >= 2 => {
                        let x = self.expr(&rest[0..1]);
                        let y = self.expr(&rest[1..2]);
                        Instr::Ucontrol(UcOp::Pathfind(x, y))
                    }
                    _ => {
                        self.warn_unsupported(&format!("ucontrol {sub}"));
                        Instr::NoOp
                    }
                }
            }
            "radar" => {
                // radar <from> <t1> <t2> <t3> <order> <sort> <output>
                if args.len() >= 7 {
                    let from = self.expr(&args[0..1]);
                    if let (Some(t1), Some(t2), Some(t3), Some(sort)) = (
                        RadarTarget::parse(&args[1]),
                        RadarTarget::parse(&args[2]),
                        RadarTarget::parse(&args[3]),
                        RadarSort::parse(&args[5]),
                    ) {
                        let order = self.expr(&args[4..5]);
                        let output = self.var(&args[6]);
                        return Instr::Radar(RadarSpec {
                            from,
                            targets: [t1, t2, t3],
                            sort,
                            order,
                            output,
                        });
                    }
                }
                self.warn_unsupported("radar");
                Instr::NoOp
            }
            "ulocate" => {
                if args.len() >= 6 {
                    if let Some(kind) = UlocKind::parse(&args[0]) {
                        // group is only meaningful for building/damaged finds;
                        // real-world code writes `0` for spawn/core.
                        let group = UlocGroup::parse(&args[1]).unwrap_or(UlocGroup::All);
                        let enemy = self.expr(&args[2..3]);
                        let ore = match kind {
                            UlocKind::Ore => {
                                // <ore> token is an item like @copper or "copper"
                                args.get(3).map(|token| {
                                    token.trim_start_matches('@').trim_matches('"').to_string()
                                })
                            }
                            _ => None,
                        };
                        let offset = if kind == UlocKind::Ore { 1 } else { 0 };
                        if args.len() >= 6 + offset {
                            let building = self.var(&args[3 + offset]);
                            let x = self.var(&args[4 + offset]);
                            let y = self.var(&args[5 + offset]);
                            return Instr::Ulocate(UlocSpec {
                                kind,
                                group,
                                enemy,
                                ore,
                                building,
                                x,
                                y,
                            });
                        }
                    }
                }
                self.warn_unsupported("ulocate");
                Instr::NoOp
            }
            "setflag" => {
                if args.len() >= 2 {
                    let flag = self.flag_key(&args[0]);
                    let value = self.expr(&args[1..]);
                    return Instr::SetFlag(flag, value);
                }
                Instr::NoOp
            }
            "lookup" => {
                // lookup <type> <index> <result>
                if args.len() >= 3 {
                    if let Some(kind) = LookupKind::parse(&args[0]) {
                        let index = self.expr(&args[1..2]);
                        let result = self.var(&args[2]);
                        return Instr::Lookup(kind, index, result);
                    }
                }
                self.warn_unsupported("lookup");
                Instr::NoOp
            }
            "fetch" => {
                // JAR 158.1 `LogicIO.read/write` + FetchStatement:
                // fetch <type> <result> <team> <index> [extra]
                // (team defaults to @sharded, index to 0; the older
                // 4-token form `fetch unit result 0` parsed team=0 and is
                // NOT the 158.1 grammar).
                if args.len() >= 2 {
                    if let Some(kind) = FetchKind::parse(&args[0]) {
                        let result = self.var(&args[1]);
                        let team = args
                            .get(2)
                            .map(|token| self.team_expr(token))
                            .unwrap_or(Expr::Num(1.0)); // @sharded default
                        let index = args
                            .get(3)
                            .map(|token| self.expr(std::slice::from_ref(token)))
                            .unwrap_or(Expr::Num(0.0));
                        // The extra operand is a CONTENT OBJECT at runtime
                        // (FetchI checks `extra.obj() instanceof UnitType /
                        // Block`); keep literal tokens as strings so the
                        // executor can classify them. Official default:
                        // FetchStatement.extra = "@conveyor" (a Block, so a
                        // unit fetch never filters).
                        let extra = args
                            .get(4)
                            .map(|token| self.expr(std::slice::from_ref(token)))
                            .unwrap_or_else(|| Expr::Str("@conveyor".to_string()));
                        return Instr::Fetch(kind, result, team, index, extra);
                    }
                }
                self.warn_unsupported("fetch");
                Instr::NoOp
            }
            "format" => {
                // JAR 158.1 FormatStatement: `format <value>` — a single
                // operand substituted into the runtime print buffer
                // (FormatI.run scans the textBuffer for the LAST `{n}`).
                if !args.is_empty() {
                    let value = self.expr(&args[0..1]);
                    return Instr::Format(value);
                }
                self.warn_unsupported("format");
                Instr::NoOp
            }
            "getblock" => {
                // JAR 158.1 GetBlockStatement: `getblock <layer> <result>
                // <x> <y>` — tile coordinates, privileged-only.
                if args.len() >= 4 {
                    if let Some(layer) = TileLayer::parse(&args[0]) {
                        let result = self.var(&args[1]);
                        let x = self.expr(&args[2..3]);
                        let y = self.expr(&args[3..4]);
                        return Instr::GetBlock(layer, result, x, y);
                    }
                }
                self.warn_unsupported("getblock");
                Instr::NoOp
            }
            "setblock" => {
                // JAR 158.1 SetBlockStatement: `setblock <layer> <block>
                // <x> <y> <team> <rotation>` — tile coordinates,
                // privileged-only.
                if args.len() >= 6 {
                    if let Some(layer) = TileLayer::parse(&args[0]) {
                        let block = self.content_expr(&args[1]);
                        let x = self.expr(&args[2..3]);
                        let y = self.expr(&args[3..4]);
                        let team = self.team_expr(&args[4]);
                        let rotation = self.expr(&args[5..6]);
                        return Instr::SetBlock(layer, block, x, y, team, rotation);
                    }
                }
                self.warn_unsupported("setblock");
                Instr::NoOp
            }
            "getflag" => {
                if args.len() >= 2 {
                    let dest = self.var(&args[0]);
                    let flag = self.flag_key(&args[1]);
                    return Instr::GetFlag(dest, flag);
                }
                Instr::NoOp
            }
            "spawn" => {
                // Official v159.7: spawn type x y rotation team result effect
                // Legacy 5-arg: type x y rotation result (team=@sharded, effect=true)
                // Legacy 6-arg with trailing effect: type x y rotation result effect
                // Legacy 6-arg with team: type x y rotation team result (effect=true)
                // `effect` is always a runtime Expr (LVar), never a compile-time bool.
                if args.len() >= 5 {
                    let unit_type = self.unit_type_expr(&args[0]);
                    let default_effect = || Expr::Num(1.0);
                    let (x, y, rotation, team, result, effect) = if args.len() >= 7 {
                        (
                            self.expr(&args[1..2]),
                            self.expr(&args[2..3]),
                            self.expr(&args[3..4]),
                            self.team_expr(&args[4]),
                            self.var(&args[5]),
                            self.expr(&args[6..7]),
                        )
                    } else if args.len() >= 6 {
                        let trailing_effect =
                            matches!(args[5].as_str(), "true" | "false" | "0" | "1");
                        if trailing_effect {
                            (
                                self.expr(&args[1..2]),
                                self.expr(&args[2..3]),
                                self.expr(&args[3..4]),
                                self.team_expr("@sharded"),
                                self.var(&args[4]),
                                self.expr(&args[5..6]),
                            )
                        } else {
                            (
                                self.expr(&args[1..2]),
                                self.expr(&args[2..3]),
                                self.expr(&args[3..4]),
                                self.team_expr(&args[4]),
                                self.var(&args[5]),
                                default_effect(),
                            )
                        }
                    } else {
                        (
                            self.expr(&args[1..2]),
                            self.expr(&args[2..3]),
                            self.expr(&args[3..4]),
                            self.team_expr("@sharded"),
                            self.var(&args[4]),
                            default_effect(),
                        )
                    };
                    return Instr::Spawn(SpawnSpec {
                        unit_type,
                        x,
                        y,
                        rotation,
                        team,
                        result,
                        effect,
                    });
                }
                self.warn_unsupported("spawn");
                Instr::NoOp
            }
            "uradar" => {
                // Official LogicIO UnitRadarStatement field order:
                // t1 t2 t3 sort radar(unused) sortOrder output. Always
                // scans from `@unit` (UnitRadarStatement.build).
                if args.len() >= 7 {
                    if let (Some(t1), Some(t2), Some(t3), Some(sort)) = (
                        RadarTarget::parse(&args[0]),
                        RadarTarget::parse(&args[1]),
                        RadarTarget::parse(&args[2]),
                        RadarSort::parse(&args[3]),
                    ) {
                        let from = self.expr(&["@unit".to_string()]);
                        let order = self.expr(&args[5..6]);
                        let output = self.var(&args[6]);
                        return Instr::Radar(RadarSpec {
                            from,
                            targets: [t1, t2, t3],
                            sort,
                            order,
                            output,
                        });
                    }
                }
                // Handwritten 6-arg form without the unused radar field.
                if args.len() >= 6 {
                    if let (Some(t1), Some(t2), Some(t3), Some(sort)) = (
                        RadarTarget::parse(&args[0]),
                        RadarTarget::parse(&args[1]),
                        RadarTarget::parse(&args[2]),
                        RadarSort::parse(&args[3]),
                    ) {
                        let from = self.expr(&["@unit".to_string()]);
                        let order = self.expr(&args[4..5]);
                        let output = self.var(&args[5]);
                        return Instr::Radar(RadarSpec {
                            from,
                            targets: [t1, t2, t3],
                            sort,
                            order,
                            output,
                        });
                    }
                }
                self.warn_unsupported("uradar");
                Instr::NoOp
            }
            "status" => {
                // Official LogicIO ApplyStatusStatement:
                // status <clear> <effect> <unit> <duration>
                if args.len() >= 3 {
                    if let Some(clear) = parse_status_clear(&args[0]) {
                        let effect = args[1]
                            .trim_start_matches('@')
                            .trim_matches('"')
                            .to_string();
                        let unit = self.expr(&args[2..3]);
                        let duration = if args.len() >= 4 {
                            self.expr(&args[3..4])
                        } else {
                            Expr::Num(10.0)
                        };
                        return Instr::ApplyStatus(ApplyStatusSpec {
                            clear,
                            effect,
                            unit,
                            duration,
                        });
                    }
                }
                self.warn_unsupported("status");
                Instr::NoOp
            }
            "spawnwave" => {
                // Official LogicIO SpawnWaveStatement: x y natural
                if args.len() >= 3 {
                    let x = self.expr(&args[0..1]);
                    let y = self.expr(&args[1..2]);
                    let natural = self.expr(&args[2..3]);
                    return Instr::SpawnWave(natural, x, y);
                }
                self.warn_unsupported("spawnwave");
                Instr::NoOp
            }
            "setrule" => {
                // Official LogicIO SetRuleStatement:
                // setrule <rule> <value> [p1 p2 p3 p4]
                if args.len() >= 2 {
                    if let Some(rule) = LogicRule::parse(&args[0]) {
                        let value = if matches!(rule, LogicRule::Ban | LogicRule::Unban) {
                            let name = args[1]
                                .trim_start_matches('@')
                                .trim_matches('"')
                                .to_string();
                            Expr::Str(name)
                        } else {
                            self.expr(&args[1..2])
                        };
                        let p1 = args
                            .get(2)
                            .map(|token| self.team_expr(token))
                            .unwrap_or(Expr::Num(0.0));
                        let p2 = args
                            .get(3)
                            .map(|token| self.expr(std::slice::from_ref(token)))
                            .unwrap_or(Expr::Num(0.0));
                        let p3 = args
                            .get(4)
                            .map(|token| self.expr(std::slice::from_ref(token)))
                            .unwrap_or(Expr::Num(100.0));
                        let p4 = args
                            .get(5)
                            .map(|token| self.expr(std::slice::from_ref(token)))
                            .unwrap_or(Expr::Num(100.0));
                        return Instr::SetRule(SetRuleSpec {
                            rule,
                            value,
                            p1,
                            p2,
                            p3,
                            p4,
                        });
                    }
                }
                self.warn_unsupported("setrule");
                Instr::NoOp
            }
            "explosion" => {
                // Official LogicIO ExplosionStatement:
                // explosion team x y radius damage air ground pierce effect
                if args.len() >= 8 {
                    return Instr::Explosion(ExplosionSpec {
                        team: self.team_expr(&args[0]),
                        x: self.expr(&args[1..2]),
                        y: self.expr(&args[2..3]),
                        radius: self.expr(&args[3..4]),
                        damage: self.expr(&args[4..5]),
                        air: self.expr(&args[5..6]),
                        ground: self.expr(&args[6..7]),
                        pierce: self.expr(&args[7..8]),
                    });
                }
                self.warn_unsupported("explosion");
                Instr::NoOp
            }
            "setprop" => {
                // Official LogicIO SetPropStatement: setprop <type> <of> <value>
                if args.len() >= 3 {
                    if let Some(key) = parse_setprop_key(&args[0]) {
                        let of = self.expr(&args[1..2]);
                        let value = self.expr(&args[2..3]);
                        return Instr::SetProp(SetPropSpec { key, of, value });
                    }
                }
                self.warn_unsupported("setprop");
                Instr::NoOp
            }
            "end" => Instr::End,
            "label" => Instr::NoOp,
            _ => {
                self.warn_unsupported(name);
                Instr::NoOp
            }
        }
    }

    /// Flag key: quoted string -> content, otherwise trim `@` and quotes.
    pub(crate) fn flag_key(&self, token: &str) -> Expr {
        let key = token.trim_matches('"').trim_start_matches('@').to_string();
        Expr::Str(key)
    }

    fn warn_unsupported(&mut self, name: &str) {
        if self.warned.insert(name.to_string()) {
            log::warn!(
                "logic: statement '{name}' is not supported yet (phase 1); compiled to NoOp"
            );
        }
        // The located diagnostic is appended by the pass-2 NoOp detector.
    }

    /// Parses an expression from tokens (precedence-climbing).
    fn expr(&mut self, tokens: &[String]) -> Expr {
        let mut pos = 0;
        let parsed = self.parse_expr_bp(tokens, &mut pos, 0);
        // Ignore trailing tokens (should not happen for well-formed code).
        parsed
    }

    fn parse_expr_bp(&mut self, tokens: &[String], pos: &mut usize, min_bp: u8) -> Expr {
        let mut left = self.parse_primary(tokens, pos);
        while *pos < tokens.len() {
            let op_token = tokens[*pos].as_str();
            let Some((op, l_bp, r_bp)) = binary_op_info(op_token) else {
                break;
            };
            if l_bp < min_bp {
                break;
            }
            *pos += 1;
            let right = self.parse_expr_bp(tokens, pos, r_bp);
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        left
    }

    fn parse_primary(&mut self, tokens: &[String], pos: &mut usize) -> Expr {
        if *pos >= tokens.len() {
            return Expr::Num(0.0);
        }
        let token = tokens[*pos].clone();
        *pos += 1;
        if token == "(" {
            let inner = self.parse_expr_bp(tokens, pos, 0);
            if *pos < tokens.len() && tokens[*pos] == ")" {
                *pos += 1;
            }
            return inner;
        }
        // unary minus
        if token == "-" && *pos < tokens.len() {
            let inner = self.parse_expr_bp(tokens, pos, 9);
            return Expr::Bin(Op::Sub, Box::new(Expr::Num(0.0)), Box::new(inner));
        }
        if token == "!" && *pos < tokens.len() {
            let inner = self.parse_expr_bp(tokens, pos, 9);
            return Expr::Bin(Op::Not, Box::new(Expr::Num(0.0)), Box::new(inner));
        }
        if token.len() >= 2 && token.starts_with('"') && token.ends_with('"') {
            return Expr::Str(token[1..token.len() - 1].replace("\\n", "\n"));
        }
        // Official GlobalVars constants (not allocated as user variables).
        match token.as_str() {
            "true" => return Expr::Num(1.0),
            "false" => return Expr::Num(0.0),
            "@pi" | "π" => return Expr::Num(std::f64::consts::PI),
            _ => {}
        }
        if let Some(value) = parse_number(&token.replace(' ', "_")) {
            return Expr::Num(value);
        }
        if !self.name_map.contains_key(&token) {
            let name = token.trim().trim_start_matches('@').trim_matches('"');
            if let Some(id) = crate::network::units::parse_unit_type(name) {
                return Expr::UnitType(id);
            }
            if let Some(team) = crate::logic::ops::team_id_from_token(&token) {
                return Expr::Team(team as u8);
            }
            if let Some(block_id) = crate::game::block_names::block_id_from_name(name) {
                return Expr::Num(f64::from(block_id));
            }
        }
        let idx = self.var(&token);
        Expr::Var(idx)
    }
}

fn binary_op_info(token: &str) -> Option<(Op, u8, u8)> {
    Some(match token {
        "||" => (Op::Land, 1, 2),
        "&&" => (Op::And, 3, 4),
        "==" => (Op::Equal, 5, 6),
        "!=" => (Op::NotEqual, 5, 6),
        "===" => (Op::StrictEqual, 5, 6),
        "<" => (Op::LessThan, 7, 8),
        "<=" => (Op::LessThanEq, 7, 8),
        ">" => (Op::GreaterThan, 7, 8),
        ">=" => (Op::GreaterThanEq, 7, 8),
        "+" => (Op::Add, 9, 10),
        "-" => (Op::Sub, 9, 10),
        "*" => (Op::Mul, 11, 12),
        "/" => (Op::Div, 11, 12),
        "%" => (Op::Mod, 11, 12),
        "^" => (Op::Pow, 13, 14),
        "<<" => (Op::Shl, 9, 10),
        ">>" => (Op::Shr, 9, 10),
        _ => return None,
    })
}

fn parse_op(token: &str) -> Option<Op> {
    Some(match token {
        "add" => Op::Add,
        "sub" => Op::Sub,
        "mul" => Op::Mul,
        "div" => Op::Div,
        "idiv" => Op::Idiv,
        "mod" => Op::Mod,
        "emod" => Op::Emod,
        "pow" => Op::Pow,
        "equal" => Op::Equal,
        "notEqual" => Op::NotEqual,
        "land" => Op::Land,
        "lessThan" => Op::LessThan,
        "lessThanEq" => Op::LessThanEq,
        "greaterThan" => Op::GreaterThan,
        "greaterThanEq" => Op::GreaterThanEq,
        "strictEqual" => Op::StrictEqual,
        "shl" => Op::Shl,
        "shr" => Op::Shr,
        "ushr" => Op::Ushr,
        "or" => Op::Or,
        "and" => Op::And,
        "xor" => Op::Xor,
        "not" => Op::Not,
        "max" => Op::Max,
        "min" => Op::Min,
        "angle" => Op::Angle,
        "angleDiff" => Op::AngleDiff,
        "len" => Op::Len,
        "noise" => Op::Noise,
        "abs" => Op::Abs,
        "sign" => Op::Sign,
        "log" => Op::Log,
        "logn" => Op::Logn,
        "log10" => Op::Log10,
        "floor" => Op::Floor,
        "ceil" => Op::Ceil,
        "round" => Op::Round,
        "sqrt" => Op::Sqrt,
        "rand" => Op::Rand,
        "sin" => Op::Sin,
        "cos" => Op::Cos,
        "tan" => Op::Tan,
        "asin" => Op::Asin,
        "acos" => Op::Acos,
        "atan" => Op::Atan,
        _ => return None,
    })
}

fn parse_cond(token: &str) -> Option<Cond> {
    Some(match token {
        "==" | "equal" => Cond::Equal,
        "!=" | "notEqual" | "not" => Cond::NotEqual,
        "<" | "lessThan" => Cond::LessThan,
        "<=" | "lessThanEq" => Cond::LessThanEq,
        ">" | "greaterThan" => Cond::GreaterThan,
        ">=" | "greaterThanEq" => Cond::GreaterThanEq,
        "===" | "strictEqual" => Cond::StrictEqual,
        "always" => Cond::Always,
        _ => return None,
    })
}

fn parse_number(token: &str) -> Option<f64> {
    if token.is_empty() {
        return None;
    }
    if let Some(rest) = token
        .strip_prefix("0x")
        .or_else(|| token.strip_prefix("+0x"))
    {
        if let Ok(v) = i64::from_str_radix(rest, 16) {
            return Some(v as f64);
        }
    }
    if let Some(rest) = token.strip_prefix("-0x") {
        if let Ok(v) = i64::from_str_radix(rest, 16) {
            return Some(-(v as f64));
        }
    }
    if let Some(rest) = token
        .strip_prefix("0b")
        .or_else(|| token.strip_prefix("+0b"))
    {
        if let Ok(v) = i64::from_str_radix(rest, 2) {
            return Some(v as f64);
        }
    }
    if let Some(rest) = token.strip_prefix("-0b") {
        if let Ok(v) = i64::from_str_radix(rest, 2) {
            return Some(-(v as f64));
        }
    }
    token.parse::<f64>().ok()
}

fn parse_i32(token: &str) -> Option<i32> {
    token.parse::<i32>().ok()
}

fn parse_status_clear(token: &str) -> Option<bool> {
    Some(match token {
        "true" | "1" | "clear" => true,
        "false" | "0" | "apply" => false,
        _ => return None,
    })
}

fn parse_setprop_key(token: &str) -> Option<SetPropKey> {
    let name = token.trim().trim_start_matches('@').trim_matches('"');
    if let Some(access) = LAccess::parse(name) {
        return Some(SetPropKey::Access(access));
    }
    if let Some(item) = try_item_id_from_name(name) {
        return Some(SetPropKey::Item(item));
    }
    if let Some(liquid) = liquid_id_from_name(name) {
        return Some(SetPropKey::Liquid(liquid));
    }
    None
}

/// Splits a line into tokens, respecting double-quoted strings.
fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    for ch in line.chars() {
        if in_string {
            current.push(ch);
            if ch == '"' {
                in_string = false;
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        match ch {
            '"' => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                current.push(ch);
                in_string = true;
            }
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Extracts the source code string from a compressed processor config
/// (LogicBlock.compress container), if valid.
pub fn source_from_config(config: &[u8]) -> Option<String> {
    // P1: single bounded grammar — delegate to parse_logic_container.
    crate::logic::parse_logic_container(config).map(|container| container.source)
}

/// Parses the relative link coordinates from a processor config container.
pub fn parse_links(config: &[u8]) -> Vec<(i16, i16)> {
    // P1: single bounded grammar — delegate to parse_logic_container. The
    // official link cap is LogicBlock.maxLinks (6000), not the old 64.
    crate::logic::parse_logic_container(config)
        .map(|container| container.links)
        .unwrap_or_default()
}

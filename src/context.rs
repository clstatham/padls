use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use anyhow::{Error, Result};
use rustc_hash::FxHashMap;

use crate::{
    bit::Bit,
    op::Op,
    parser::{parse_defs, BinaryOperator, Def, Expr},
};

pub struct TaggedOp {
    id: usize,
    op: Arc<Op>,
}

impl TaggedOp {
    pub fn id(&self) -> usize {
        self.id
    }
}

pub struct ExecutionContext {
    name: String,
    next_id: AtomicUsize,
    inputs: Vec<usize>,
    pool: FxHashMap<usize, TaggedOp>,
    output: Option<usize>,
}

impl ExecutionContext {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            inputs: Vec::new(),
            next_id: AtomicUsize::new(0),
            pool: FxHashMap::default(),
            output: None,
        }
    }

    fn parse_expr(&mut self, expr: &Expr, tag_map: &mut FxHashMap<String, usize>) -> Result<usize> {
        let op_id = match expr {
            Expr::Binding(binding) => *tag_map.get(&binding.0).unwrap(),
            Expr::Not(not) => {
                let expr_id = self.parse_expr(&not.expr, tag_map)?;
                let mut op = Op::new(Arc::new(move |args| !args[0]));
                op.connect_input(self.pool.get(&expr_id).unwrap().op.clone());
                self.push(Arc::new(op))
            }
            Expr::BinaryExpr(expr) => {
                let a_id = self.parse_expr(&expr.a, tag_map)?;
                let b_id = self.parse_expr(&expr.b, tag_map)?;
                let mut op = match expr.op {
                    BinaryOperator::And => Op::new(Arc::new(move |args| args[0] & args[1])),
                    BinaryOperator::Or => Op::new(Arc::new(move |args| args[0] | args[1])),
                    BinaryOperator::Xor => Op::new(Arc::new(move |args| args[0] ^ args[1])),
                };
                op.connect_input(self.pool.get(&a_id).unwrap().op.clone());
                op.connect_input(self.pool.get(&b_id).unwrap().op.clone());
                self.push(Arc::new(op))
            }
        };
        Ok(op_id)
    }

    pub fn parse_def(def: &Def, inputs: &[Arc<Op>]) -> Result<Self> {
        let mut tag_map = FxHashMap::default();
        let mut this = Self::new(&def.name);
        for (binding, inp) in def.inputs.iter().zip(inputs.iter()) {
            let id = this.add_input(inp.clone());
            tag_map.insert(binding.0.to_owned(), id);
        }
        let out_id = this.parse_expr(&def.logic, &mut tag_map)?;
        this.output = Some(out_id);
        Ok(this)
    }

    pub fn parse_script(script: &str, inputs: Vec<Vec<Arc<Op>>>) -> Result<Vec<Self>> {
        let mut out = vec![];
        let script = script.lines().collect::<Vec<_>>().join(" ");
        let (_junk, defs) =
            parse_defs(&script).map_err(|e| Error::msg(format!("Parsing error: {:?}", e)))?;
        for ((_, def), inps) in defs.iter().zip(inputs.iter()) {
            out.push(Self::parse_def(def, inps)?);
        }
        Ok(out)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn root(&self) -> usize {
        0
    }

    pub fn input_ids(&self) -> &[usize] {
        &self.inputs
    }

    pub fn output_id(&self) -> Option<usize> {
        self.output
    }

    pub fn inputs(&self) -> Vec<Arc<Op>> {
        self.inputs
            .iter()
            .map(|id| self.pool.get(id).unwrap().op.clone())
            .collect()
    }

    fn alloc_id(&self) -> usize {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    pub fn connect(&mut self, inputs: Vec<usize>, mut op: Op) -> Result<usize> {
        if !op.args.is_empty() {
            return Err(Error::msg("Op already had args connected"));
        }

        for inp in inputs {
            op.args.push(
                self.pool
                    .get(&inp)
                    .ok_or(Error::msg(format!(
                        "Couldn't find op id {inp} in pool for connection"
                    )))?
                    .op
                    .clone(),
            );
        }
        Ok(self.push(Arc::new(op)))
    }

    pub fn push(&mut self, op: Arc<Op>) -> usize {
        let id = self.alloc_id();
        self.pool.insert(id, TaggedOp { id, op });
        id
    }

    pub fn add_input(&mut self, inp: Arc<Op>) -> usize {
        let id = self.push(inp);
        self.inputs.push(id);
        id
    }

    pub async fn exec(&mut self) -> Bit {
        let out = self.pool.get(&self.output.unwrap()).unwrap();
        out.op.clone().eval().await
    }
}

#[cfg(test)]
mod tests {
    use crate::bit::Bit;

    use super::ExecutionContext;

    #[test]
    fn test_parse_context() {
        let script = include_str!("parser/test_scripts/test1.pals");
        let ctxs =
            ExecutionContext::parse_script(script, vec![vec![Bit::hi().into(), Bit::hi().into()]])
                .unwrap();
        for ctx in ctxs {
            println!("Context name: {}", ctx.name());
            println!("Context input IDs: {:?}", ctx.input_ids());
            println!("Context output ID: {}", ctx.output_id().unwrap());
        }
    }
}

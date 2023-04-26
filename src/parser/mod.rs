use nom::branch::*;
use nom::bytes::complete::tag;
use nom::character::complete::*;
use nom::combinator::*;
use nom::multi::*;
use nom::sequence::*;
use nom::IResult;

use rustc_hash::FxHashSet;

#[derive(Debug, Clone)]
pub enum BinaryOperator {
    And,
    Or,
    Xor,
}

#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub enum Binding {
    Named(String),
    Clk,
    Hi,
    Lo,
}

impl std::fmt::Display for Binding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Clk => write!(f, "clk"),
            Self::Named(name) => write!(f, "{name}"),
            Self::Hi => write!(f, "hi"),
            Self::Lo => write!(f, "lo"),
            // Self::InternalHidden(_) => Ok(()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Not {
    pub expr: Box<Expr>,
}

#[derive(Debug, Clone)]
pub struct CircuitCall {
    pub name: String,
    pub inputs: Vec<Expr>,
}

#[derive(Debug, Clone)]
pub enum Expr {
    CircuitCall(CircuitCall),
    BinaryExpr(BinaryExpr),
    Not(Not),
    Binding(Binding),
}

impl Expr {
    pub fn all_refs(&self) -> FxHashSet<Binding> {
        match self {
            Self::Not(not) => not.expr.all_refs(),
            Self::Binding(bind) => FxHashSet::from_iter([bind.to_owned()]),
            Self::BinaryExpr(expr) => {
                let mut out = expr.a.all_refs();
                out.extend(expr.b.all_refs());
                out
            }
            Self::CircuitCall(call) => {
                let mut out = FxHashSet::default();
                for inp in call.inputs.iter() {
                    out.extend(inp.all_refs());
                }
                out
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct BinaryExpr {
    pub a: Box<Expr>,
    pub b: Box<Expr>,
    pub op: BinaryOperator,
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub targets: Vec<Binding>,
    pub expr: Expr,
}

impl Assignment {
    pub fn all_refs(&self) -> FxHashSet<Binding> {
        let mut out = FxHashSet::from_iter(self.targets.iter().cloned());
        out.extend(self.expr.all_refs());
        out
    }
}

#[derive(Debug, Clone)]
pub struct Circuit {
    pub name: String,
    pub inputs: Vec<Binding>,
    pub outputs: Vec<Binding>,
    pub logic: Vec<Assignment>,
}

pub fn binding(i: &str) -> IResult<&str, Binding> {
    map(
        recognize(many1(alt((alphanumeric1, tag("_"))))),
        |res: &str| {
            // if res.0.is_some() {
            match res {
                "clk" => Binding::Clk,
                "hi" => Binding::Hi,
                "lo" => Binding::Lo,
                _ => Binding::Named(res.to_owned()),
            }
        },
    )(i)
}

pub fn binary_operator(i: &str) -> IResult<&str, BinaryOperator> {
    map(recognize(one_of("&|^")), |s: &str| match s {
        "&" => BinaryOperator::And,
        "|" => BinaryOperator::Or,
        "^" => BinaryOperator::Xor,
        _ => unreachable!(),
    })(i)
}

pub fn def_call(i: &str) -> IResult<&str, CircuitCall> {
    map(
        tuple((
            circuit_name,
            tag("["),
            many1(terminated(expr, opt(space1))),
            tag("]"),
        )),
        |(name, _, inputs, _)| CircuitCall {
            name: name.to_owned(),
            inputs,
        },
    )(i)
}

pub fn expr(i: &str) -> IResult<&str, Expr> {
    alt((
        map(def_call, Expr::CircuitCall),
        map(
            tuple((
                tag("("),
                space0,
                expr,
                space1,
                binary_operator,
                space1,
                expr,
                space0,
                tag(")"),
            )),
            |(_, _, a, _, op, _, b, _, _)| {
                Expr::BinaryExpr(BinaryExpr {
                    a: Box::new(a),
                    b: Box::new(b),
                    op,
                })
            },
        ),
        map(tuple((tag("!"), expr)), |(_, a)| {
            Expr::Not(Not { expr: Box::new(a) })
        }),
        map(binding, Expr::Binding),
    ))(i)
}

pub fn circuit_name(i: &str) -> IResult<&str, &str> {
    recognize(many1(alt((alphanumeric1, tag("_")))))(i)
}

pub fn circuit(i: &str) -> IResult<&str, Circuit> {
    map(
        tuple((
            terminated(circuit_name, opt(space1)),
            preceded(
                tag("["),
                terminated(many0(terminated(binding, opt(space1))), tag("]")),
            ),
            space1,
            tag("->"),
            space1,
            many1(terminated(binding, opt(space1))),
            tag("{"),
            space1,
            many1(tuple((
                many1(terminated(binding, space1)),
                tag("<-"),
                space1,
                terminated(expr, tuple((tag(";"), opt(space1)))),
            ))),
            tag("}"),
        )),
        |(name, inputs, _, _, _, outputs, _, _, logic, _)| {
            let logic = logic
                .into_iter()
                .map(|(targets, _, _, expr)| Assignment { targets, expr })
                .collect();
            Circuit {
                name: name.to_owned(),
                inputs,
                outputs,
                logic,
            }
        },
    )(i)
}

pub fn parse_circuits(i: &str) -> IResult<&str, Vec<Circuit>> {
    many1(terminated(circuit, opt(space1)))(i)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_parse_1() {
        let script = include_str!("test_scripts/full_adder.pals");
        let script = script.lines().collect::<Vec<_>>().join(" ");
        dbg!(super::parse_circuits(&script).unwrap());
    }
}

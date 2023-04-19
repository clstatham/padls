use nom::branch::*;
use nom::bytes::complete::tag;
use nom::character::complete::*;
use nom::combinator::*;
use nom::multi::*;
use nom::sequence::*;
use nom::IResult;
use rustc_hash::FxHashMap;

#[derive(Debug)]
pub enum BinaryOperator {
    And,
    Or,
    Xor,
}

#[derive(Debug)]
pub struct Binding(pub String);

#[derive(Debug)]
pub struct Not {
    pub expr: Box<Expr>,
}

#[derive(Debug)]
pub enum Expr {
    BinaryExpr(BinaryExpr),
    Not(Not),
    Binding(Binding),
}

#[derive(Debug)]
pub struct BinaryExpr {
    pub a: Box<Expr>,
    pub b: Box<Expr>,
    pub op: BinaryOperator,
}

#[derive(Debug)]
pub struct Def {
    pub name: String,
    pub inputs: Vec<Binding>,
    // pub outputs: Vec<Output>,
    pub logic: Expr,
}

pub fn binding(i: &str) -> IResult<&str, Binding> {
    map(
        recognize(many1(alt((alphanumeric1, tag("_"))))),
        |s: &str| Binding(s.to_owned()),
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

pub fn expr(i: &str) -> IResult<&str, Expr> {
    alt((
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

// pub fn input(i: &str) -> IResult<&str, Binding> {
//     map(tuple((tag("::"), space1, binding)), |(_, _, binding)| {
//         binding
//     })(i)
// }

pub fn def_name(i: &str) -> IResult<&str, &str> {
    recognize(alphanumeric1)(i)
}

pub fn def(i: &str) -> IResult<&str, Def> {
    map(
        tuple((
            terminated(def_name, space1),
            terminated(tag("::"), space1),
            many1(terminated(binding, space1)),
            // many1(terminated(output, space1)),
            tag("{"),
            space1,
            terminated(expr, space1),
            tag("}"),
        )),
        |(name, _, inputs, _, _, logic, _)| Def {
            name: name.to_owned(),
            inputs,
            // outputs,
            logic,
        },
    )(i)
}

pub fn parse_defs(i: &str) -> IResult<&str, FxHashMap<String, Def>> {
    map(many1(terminated(def, opt(space1))), |defs| {
        let mut map = FxHashMap::default();
        for def in defs {
            map.insert(def.name.to_owned(), def);
        }
        map
    })(i)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_parse_1() {
        let script = include_str!("test_scripts/test1.pals");
        let script = script.lines().collect::<Vec<_>>().join(" ");
        dbg!(super::parse_defs(&script).unwrap());
    }
}

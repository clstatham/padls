use ariadne::{ColorGenerator, Label, Report, Source};
use logos::{Lexer, Logos, Span};
use petgraph::{matrix_graph::DiMatrix, prelude::*};
use rustc_hash::FxHashMap;

use crate::gates::Gate;

type Error = (String, Span);
type Result<T> = std::result::Result<T, Error>;

#[derive(Logos, Debug, Clone, PartialEq, Eq)]
#[logos(skip r"[ \t\r\n\f]+")]
pub enum Token {
    #[regex(r"[a-zA-Z_][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),

    #[token("!")]
    Not,

    #[token("&")]
    And,

    #[token("|")]
    Or,

    #[token("^")]
    Xor,

    #[token("(")]
    LParen,

    #[token(")")]
    RParen,

    #[token("->")]
    RArrow,

    #[token("<-")]
    LArrow,

    #[token("[")]
    LBracket,

    #[token("]")]
    RBracket,

    #[token("{")]
    LBrace,

    #[token("}")]
    RBrace,

    #[token(",")]
    Comma,

    #[token(";")]
    Semicolon,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Wire {
    pub source_output: u32,
    pub target_input: u32,
}

#[derive(Clone)]
pub struct Circuit {
    pub name: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub graph: DiMatrix<Gate, Wire, Option<Wire>, u32>,
    pub bindings: FxHashMap<String, NodeIndex>,
}

impl Circuit {
    pub fn binding(&mut self, name: String) -> NodeIndex {
        if let Some(&index) = self.bindings.get(&name) {
            return index;
        }
        let index = self.graph.add_node(Gate::Identity);
        self.bindings.insert(name, index);
        index
    }

    pub fn connect(
        &mut self,
        source: NodeIndex,
        target: NodeIndex,
        source_output: u32,
        target_input: u32,
    ) {
        let wire = Wire {
            source_output,
            target_input,
        };
        self.graph.add_edge(source, target, wire);
    }

    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }
}

pub struct Parser<'a> {
    lexer: Lexer<'a, Token>,
    circuits: Vec<Circuit>,
}

impl<'a> Parser<'a> {
    pub fn next_token(&mut self) -> Option<std::result::Result<Token, ()>> {
        self.lexer.next()
    }

    pub fn span(&self) -> Span {
        self.lexer.span()
    }

    pub fn expect(&mut self, token: Token) -> Result<()> {
        match self.next_token() {
            Some(Ok(t)) if t == token => Ok(()),
            Some(Ok(t)) => Err((format!("expected {:?}, found {:?}", token, t), self.span())),
            Some(Err(())) => Err((format!("expected {:?}, found error", token), self.span())),
            None => Err((format!("expected {:?}, found EOF", token), self.span())),
        }
    }

    pub fn parse_ident(&mut self) -> Result<String> {
        match self.next_token() {
            Some(Ok(Token::Ident(ident))) => Ok(ident),
            Some(Ok(token)) => Err((
                format!("expected identifier, found {:?}", token),
                self.span(),
            )),
            Some(Err(())) => Err(("expected identifier, found error".to_string(), self.span())),
            None => Err(("expected identifier, found EOF".to_string(), self.span())),
        }
    }

    pub fn parse_prefix_expr(&mut self, circuit: &mut Circuit) -> Result<NodeIndex> {
        match self.next_token() {
            Some(Ok(Token::Not)) => {
                self.expect(Token::LBracket)?;
                let node = self.parse_prefix_expr(circuit)?;
                self.expect(Token::RBracket)?;
                let not = circuit.graph.add_node(Gate::Not);
                circuit.graph.add_edge(
                    node,
                    not,
                    Wire {
                        source_output: 0,
                        target_input: 0,
                    },
                );
                Ok(not)
            }
            Some(Ok(Token::And)) => {
                self.expect(Token::LBracket)?;
                let lhs = self.parse_prefix_expr(circuit)?;
                let rhs = self.parse_prefix_expr(circuit)?;
                self.expect(Token::RBracket)?;
                let and = circuit.graph.add_node(Gate::And);
                circuit.graph.add_edge(
                    lhs,
                    and,
                    Wire {
                        source_output: 0,
                        target_input: 0,
                    },
                );
                circuit.graph.add_edge(
                    rhs,
                    and,
                    Wire {
                        source_output: 0,
                        target_input: 1,
                    },
                );
                Ok(and)
            }
            Some(Ok(Token::Or)) => {
                self.expect(Token::LBracket)?;
                let lhs = self.parse_prefix_expr(circuit)?;
                let rhs = self.parse_prefix_expr(circuit)?;
                self.expect(Token::RBracket)?;
                let or = circuit.graph.add_node(Gate::Or);
                circuit.graph.add_edge(
                    lhs,
                    or,
                    Wire {
                        source_output: 0,
                        target_input: 0,
                    },
                );
                circuit.graph.add_edge(
                    rhs,
                    or,
                    Wire {
                        source_output: 0,
                        target_input: 1,
                    },
                );
                Ok(or)
            }
            Some(Ok(Token::Xor)) => {
                self.expect(Token::LBracket)?;
                let lhs = self.parse_prefix_expr(circuit)?;
                let rhs = self.parse_prefix_expr(circuit)?;
                self.expect(Token::RBracket)?;
                let xor = circuit.graph.add_node(Gate::Xor);
                circuit.graph.add_edge(
                    lhs,
                    xor,
                    Wire {
                        source_output: 0,
                        target_input: 0,
                    },
                );
                circuit.graph.add_edge(
                    rhs,
                    xor,
                    Wire {
                        source_output: 0,
                        target_input: 1,
                    },
                );
                Ok(xor)
            }
            Some(Ok(Token::Ident(ident))) => Ok(circuit.binding(ident)),
            Some(Ok(token)) => Err((
                format!("expected prefix expression, found {:?}", token),
                self.span(),
            )),
            Some(Err(())) => Err((
                "expected prefix expression, found error".to_owned(),
                self.span(),
            )),
            None => Err((
                "expected prefix expression, found EOF".to_owned(),
                self.span(),
            )),
        }
    }

    pub fn parse_circuit(&mut self) -> Result<()> {
        let name = self.parse_ident()?;
        self.expect(Token::LBracket)?;
        let mut inputs = Vec::new();
        loop {
            match self.next_token() {
                Some(Ok(Token::RBracket)) => break,
                Some(Ok(Token::Ident(ident))) => {
                    inputs.push(ident);
                }
                Some(Ok(token)) => {
                    return Err((
                        format!("expected identifier or ']', found {:?}", token),
                        self.span(),
                    ))
                }
                Some(Err(())) => {
                    return Err((
                        "expected identifier or ']', found error".to_owned(),
                        self.span(),
                    ))
                }
                None => {
                    return Err((
                        "expected identifier or ']', found EOF".to_owned(),
                        self.span(),
                    ))
                }
            }
        }

        self.expect(Token::RArrow)?;

        self.expect(Token::LBracket)?;
        let mut outputs = Vec::new();
        loop {
            match self.next_token() {
                Some(Ok(Token::RBracket)) => break,
                Some(Ok(Token::Ident(ident))) => {
                    outputs.push(ident);
                }
                Some(Ok(token)) => {
                    return Err((
                        format!("expected identifier or ']', found {:?}", token),
                        self.span(),
                    ))
                }
                Some(Err(())) => {
                    return Err((
                        "expected identifier or ']', found error".to_owned(),
                        self.span(),
                    ))
                }
                None => {
                    return Err((
                        "expected identifier or ']', found EOF".to_owned(),
                        self.span(),
                    ))
                }
            }
        }

        self.expect(Token::LBrace)?;

        let mut circ = Circuit {
            name,
            inputs,
            outputs,
            graph: DiMatrix::default(),
            bindings: FxHashMap::default(),
        };

        let inputs = circ.inputs.clone();
        let outputs = circ.outputs.clone();

        for input in inputs {
            circ.binding(input.clone());
        }

        for output in outputs {
            circ.binding(output.clone());
        }

        loop {
            match self.next_token() {
                Some(Ok(Token::RBrace)) => break,
                Some(Ok(Token::Ident(ident))) => {
                    let ident = circ.binding(ident);
                    self.expect(Token::LArrow)?;
                    let expr = self.parse_prefix_expr(&mut circ)?;
                    self.expect(Token::Semicolon)?;
                    circ.connect(expr, ident, 0, 0);
                }
                Some(Ok(token)) => {
                    return Err((
                        format!("expected identifier or '}}', found {:?}", token),
                        self.span(),
                    ))
                }
                Some(Err(())) => {
                    return Err((
                        "expected identifier or '}', found error".to_owned(),
                        self.span(),
                    ))
                }
                None => {
                    return Err((
                        "expected identifier or '}', found EOF".to_owned(),
                        self.span(),
                    ))
                }
            }
        }

        self.circuits.push(circ);

        Ok(())
    }
}

pub fn parse_circuits(input: &str) -> Result<Vec<Circuit>> {
    let lexer = Token::lexer(input);

    let mut parser = Parser {
        lexer,
        circuits: Vec::new(),
    };

    loop {
        parser.parse_circuit()?;
        if parser.lexer.remainder().is_empty() {
            break;
        }
    }

    Ok(parser.circuits)
}

pub fn parse_circuits_pretty(input: &str) -> anyhow::Result<Vec<Circuit>> {
    match parse_circuits(input) {
        Ok(circuits) => Ok(circuits),
        Err((msg, span)) => {
            let mut colors = ColorGenerator::new();

            let a = colors.next();

            Report::build(ariadne::ReportKind::Error, (), 0)
                .with_message("parse error")
                .with_label(Label::new(span).with_message(msg).with_color(a))
                .finish()
                .eprint(Source::from(input))
                .unwrap();

            Err(anyhow::anyhow!("parse error"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_circuits() {
        let input = r#"
            main [a b] -> [c d e]
            {
                c <- &[a b];
                d <- |[a b];
                foo <- ^[a bar];
                bar <- &[foo &[a b]];
                e <- ^[bar b];
            }
        "#
        .trim_end();

        let circuits = parse_circuits_pretty(input).unwrap();
        assert_eq!(circuits.len(), 1);
        let circuit = &circuits[0];
        assert_eq!(circuit.name, "main");
        assert_eq!(circuit.inputs, vec!["a", "b"]);
        assert_eq!(circuit.outputs, vec!["c", "d", "e"]);
    }
}

use ariadne::{ColorGenerator, Label, Report, Source};
use logos::{Lexer, Logos, Span};
use petgraph::{dot::Dot, matrix_graph::DiMatrix, prelude::*, visit::IntoNodeIdentifiers};
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

    pub fn dot(&self) -> String {
        let mut friendly_graph = DiMatrix::new();
        let mut node_indices = FxHashMap::default();
        for node in self.graph.node_identifiers() {
            if let Some(binding) =
                self.bindings
                    .iter()
                    .find_map(|(binding, &index)| if index == node { Some(binding) } else { None })
            {
                let friendly_node = friendly_graph.add_node(binding.to_owned());
                node_indices.insert(node, friendly_node);
            } else {
                let friendly_node = friendly_graph.add_node(format!("{:?}", self.graph[node]));
                node_indices.insert(node, friendly_node);
            }
        }

        for node in self.graph.node_identifiers() {
            for edge in self.graph.edges(node) {
                let source = node_indices[&edge.source()];
                let target = node_indices[&edge.target()];
                let weight = format!(
                    "{} -> {}",
                    edge.weight().source_output,
                    edge.weight().target_input
                );
                friendly_graph.add_edge(source, target, weight);
            }
        }

        let dot = Dot::new(&friendly_graph);
        format!("{:?}", dot)
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

    pub fn clone_circuit(&self, name: &str) -> Option<Circuit> {
        self.circuits
            .iter()
            .find(|circuit| circuit.name == name)
            .cloned()
    }

    pub fn parse_prefix_expr(&mut self, circuit: &mut Circuit) -> Result<Vec<NodeIndex>> {
        match self.next_token() {
            Some(Ok(Token::Not)) => {
                self.expect(Token::LBracket)?;
                let outs = self.parse_prefix_expr(circuit)?;
                if outs.len() != 1 {
                    return Err((
                        format!("expected single output for expr, found {:?}", outs.len()),
                        self.span(),
                    ));
                }
                self.expect(Token::RBracket)?;
                let not = circuit.graph.add_node(Gate::Not);
                circuit.graph.add_edge(
                    outs[0],
                    not,
                    Wire {
                        source_output: 0,
                        target_input: 0,
                    },
                );
                Ok(vec![not])
            }
            Some(Ok(Token::And)) => {
                self.expect(Token::LBracket)?;
                let lhs = self.parse_prefix_expr(circuit)?;
                if lhs.len() != 1 {
                    return Err((
                        format!("expected single output for expr, found {:?}", lhs.len()),
                        self.span(),
                    ));
                }
                let rhs = self.parse_prefix_expr(circuit)?;
                if rhs.len() != 1 {
                    return Err((
                        format!("expected single output for expr, found {:?}", rhs.len()),
                        self.span(),
                    ));
                }
                self.expect(Token::RBracket)?;
                let and = circuit.graph.add_node(Gate::And);
                circuit.graph.add_edge(
                    lhs[0],
                    and,
                    Wire {
                        source_output: 0,
                        target_input: 0,
                    },
                );
                circuit.graph.add_edge(
                    rhs[0],
                    and,
                    Wire {
                        source_output: 0,
                        target_input: 1,
                    },
                );
                Ok(vec![and])
            }
            Some(Ok(Token::Or)) => {
                self.expect(Token::LBracket)?;
                let lhs = self.parse_prefix_expr(circuit)?;
                if lhs.len() != 1 {
                    return Err((
                        format!("expected single output for expr, found {:?}", lhs.len()),
                        self.span(),
                    ));
                }
                let rhs = self.parse_prefix_expr(circuit)?;
                if rhs.len() != 1 {
                    return Err((
                        format!("expected single output for expr, found {:?}", rhs.len()),
                        self.span(),
                    ));
                }
                self.expect(Token::RBracket)?;
                let or = circuit.graph.add_node(Gate::Or);
                circuit.graph.add_edge(
                    lhs[0],
                    or,
                    Wire {
                        source_output: 0,
                        target_input: 0,
                    },
                );
                circuit.graph.add_edge(
                    rhs[0],
                    or,
                    Wire {
                        source_output: 0,
                        target_input: 1,
                    },
                );
                Ok(vec![or])
            }
            Some(Ok(Token::Xor)) => {
                self.expect(Token::LBracket)?;
                let lhs = self.parse_prefix_expr(circuit)?;
                if lhs.len() != 1 {
                    return Err((
                        format!("expected single output for expr, found {:?}", lhs.len()),
                        self.span(),
                    ));
                }
                let rhs = self.parse_prefix_expr(circuit)?;
                if rhs.len() != 1 {
                    return Err((
                        format!("expected single output for expr, found {:?}", rhs.len()),
                        self.span(),
                    ));
                }
                self.expect(Token::RBracket)?;
                let xor = circuit.graph.add_node(Gate::Xor);
                circuit.graph.add_edge(
                    lhs[0],
                    xor,
                    Wire {
                        source_output: 0,
                        target_input: 0,
                    },
                );
                circuit.graph.add_edge(
                    rhs[0],
                    xor,
                    Wire {
                        source_output: 0,
                        target_input: 1,
                    },
                );
                Ok(vec![xor])
            }
            Some(Ok(Token::Ident(ident))) => {
                if let Some(refd_circuit) = self.clone_circuit(&ident) {
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
                                    format!("expected identifier, found {:?}", token),
                                    self.span(),
                                ));
                            }
                            Some(Err(())) => {
                                return Err((
                                    "expected identifier, found error".to_string(),
                                    self.span(),
                                ));
                            }
                            None => {
                                return Err((
                                    "expected identifier, found EOF".to_string(),
                                    self.span(),
                                ));
                            }
                        }
                    }

                    if inputs.len() != refd_circuit.inputs.len() {
                        return Err((
                            format!(
                                "expected {:?} inputs, found {:?}",
                                refd_circuit.inputs.len(),
                                inputs.len()
                            ),
                            self.span(),
                        ));
                    }

                    let mut node_mappings = FxHashMap::default();

                    for (passed_input, circ_input) in
                        inputs.into_iter().zip(refd_circuit.inputs.iter())
                    {
                        let passed_node = circuit.binding(passed_input);
                        let circ_node = *refd_circuit.bindings.get(circ_input).unwrap();
                        node_mappings.insert(circ_node, passed_node);
                    }

                    for node in refd_circuit.graph.node_identifiers() {
                        #[allow(clippy::map_entry)]
                        if !node_mappings.contains_key(&node) {
                            let new_node = circuit.graph.add_node(refd_circuit.graph[node]);
                            node_mappings.insert(node, new_node);
                        }
                    }

                    for node in refd_circuit.graph.node_identifiers() {
                        for (source, target, weight) in refd_circuit.graph.edges(node) {
                            let source = node_mappings[&source];
                            let target = node_mappings[&target];
                            circuit.graph.update_edge(
                                source,
                                target,
                                Wire {
                                    source_output: weight.source_output,
                                    target_input: weight.target_input,
                                },
                            );
                        }
                    }

                    let mut outputs = Vec::new();
                    for output in refd_circuit.outputs.iter() {
                        let output = *refd_circuit.bindings.get(output).unwrap();
                        outputs.push(*node_mappings.get(&output).unwrap());
                    }

                    Ok(outputs)
                } else {
                    Ok(vec![circuit.binding(ident)])
                }
            }
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
                    if expr.len() != 1 {
                        return Err((
                            format!("expected single output for expr, found {:?}", expr.len()),
                            self.span(),
                        ));
                    }
                    self.expect(Token::Semicolon)?;
                    circ.connect(expr[0], ident, 0, 0);
                }
                Some(Ok(Token::LBracket)) => {
                    let mut inputs = Vec::new();
                    loop {
                        match self.next_token() {
                            Some(Ok(Token::RBracket)) => break,
                            Some(Ok(Token::Ident(ident))) => {
                                inputs.push(circ.binding(ident));
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

                    self.expect(Token::LArrow)?;

                    let outs = self.parse_prefix_expr(&mut circ)?;
                    if outs.len() != inputs.len() {
                        return Err((
                            format!(
                                "expected {} outputs for expr, found {}",
                                inputs.len(),
                                outs.len()
                            ),
                            self.span(),
                        ));
                    }

                    self.expect(Token::Semicolon)?;

                    for (out, input) in outs.iter().zip(inputs.iter()) {
                        circ.connect(*out, *input, 0, 0);
                    }
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
    let lexer = Token::lexer(input.trim_end());

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

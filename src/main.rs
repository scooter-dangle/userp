use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::Path;

use combine::{
	attempt,
	char::{char, spaces, string},
	choice,
	error::ParseError,
	many, many1, optional, parser, satisfy, sep_end_by,
	stream::state::State,
	Parser, Stream,
};
use itertools::Itertools;

// ----------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum Tree {
	Star,
	Word(String, Option<Box<Tree>>),
	Braces(Vec<Tree>),
}

#[derive(Debug, PartialEq)]
struct UseStatement {
	public: bool,
	tree: Tree,
}

fn lex_char<I>(c: char) -> impl Parser<Input = I, Output = char>
where
	I: Stream<Item = char>,
	I::Error: ParseError<I::Item, I::Range, I::Position>,
{
	char(c).skip(spaces().silent())
}

fn braces<I>() -> impl Parser<Input = I, Output = Tree>
where
	I: Stream<Item = char>,
	I::Error: ParseError<I::Item, I::Range, I::Position>,
{
	(lex_char('{'), sep_end_by(tree(), lex_char(',')), lex_char('}')).map(|(_, values, _)| Tree::Braces(values))
}

fn star<I>() -> impl Parser<Input = I, Output = Tree>
where
	I: Stream<Item = char>,
	I::Error: ParseError<I::Item, I::Range, I::Position>,
{
	lex_char('*').map(|_| Tree::Star)
}

fn word<I>() -> impl Parser<Input = I, Output = Tree>
where
	I: Stream<Item = char>,
	I::Error: ParseError<I::Item, I::Range, I::Position>,
{
	(
		many1(satisfy(|c: char| c.is_alphanumeric() || c == '_')).skip(spaces().silent()),
		optional(attempt((string("::"), tree()))),
	)
		.map(|(word, recurse)| {
			let tree = recurse.map(|(_colons, tree)| Box::new(tree));
			Tree::Word(word, tree)
		})
}

fn tree_<I>() -> impl Parser<Input = I, Output = Tree>
where
	I: Stream<Item = char>,
	I::Error: ParseError<I::Item, I::Range, I::Position>,
{
	choice((braces(), star(), word()))
}

// Fix for recursive type
parser! {
	fn tree[I]()(I) -> Tree
	where [I: Stream<Item = char>]
	{
		tree_()
	}
}

fn use_statement<I>() -> impl Parser<Input = I, Output = UseStatement>
where
	I: Stream<Item = char>,
	I::Error: ParseError<I::Item, I::Range, I::Position>,
{
	attempt((
		optional(attempt(string("pub"))).skip(spaces().silent()),
		string("use").skip(spaces().silent()),
		tree(),
		lex_char(';'),
	))
	.map(|(public, _, tree, _)| UseStatement {
		public: public.is_some(),
		tree,
	})
}

fn use_statements<I>() -> impl Parser<Input = I, Output = Vec<UseStatement>>
where
	I: Stream<Item = char>,
	I::Error: ParseError<I::Item, I::Range, I::Position>,
{
	many(use_statement())
}

// ----------------------------------------------------------------------------

#[derive(Debug, Default, PartialEq)]
struct Node(BTreeMap<String, Node>);

fn add_tree(root: &mut Node, tree: Tree) {
	match tree {
		Tree::Star => {
			root.0.insert("*".to_string(), Default::default());
		}
		Tree::Word(word, child) => {
			let entry = root.0.entry(word).or_insert_with(Default::default);
			if let Some(child) = child {
				add_tree(&mut *entry, *child);
			} else {
				entry.0.insert("self".to_string(), Default::default());
			}
		}
		Tree::Braces(trees) => {
			for tree in trees {
				add_tree(root, tree);
			}
		}
	}
}

fn into_node(statements: Vec<UseStatement>) -> (Node, Node) {
	let mut private = Default::default();
	let mut public = Default::default();
	for statement in statements {
		if statement.public {
			add_tree(&mut public, statement.tree);
		} else {
			add_tree(&mut private, statement.tree);
		}
	}
	(private, public)
}

// ----------------------------------------------------------------------------

fn format_nodes(node: Node) -> String {
	if node.0.contains_key("*") {
		"*".to_string()
	} else if node.0.len() == 1 {
		let (name, node) = node.0.into_iter().next().unwrap();
		format_mod(name, node)
	} else {
		format!(
			"{{{}}}",
			node.0
				.into_iter()
				.map(|(name, child)| format_mod(name, child))
				.format(", ")
		)
	}
}

fn format_mod(name: String, node: Node) -> String {
	if name == "self" {
		name
	} else if node.0.len() == 1 && node.0.contains_key("self") {
		name
	} else {
		format!("{}::{}", name, format_nodes(node))
	}
}

fn format_use_statements(visibility: &str, mut node: Node) -> String {
	let std = node.0.remove("std");
	let crate_ = node.0.remove("crate");
	let super_ = node.0.remove("super");

	let mut code = String::new();
	if let Some(std) = std {
		code += &format!("{}use {};\n\n", visibility, format_mod("std".to_string(), std));
	}

	// 3rd party:
	if !node.0.is_empty() {
		code += &format!("{}use {};\n\n", visibility, format_nodes(node));
	}

	if let Some(crate_) = crate_ {
		code += &format!("{}use {};\n\n", visibility, format_mod("crate".to_string(), crate_));
	}

	if let Some(super_) = super_ {
		code += &format!("{}use {};\n\n", visibility, format_mod("super".to_string(), super_));
	}

	code
}

fn prettify_code(in_code: &str) -> Result<String, String> {
	let (trees, rest_of_the_file) = use_statements()
		.easy_parse(State::new(in_code.trim()))
		.map_err(|e| e.to_string())?;
	let (private, public) = into_node(trees);
	Ok(format!(
		"{}{}{}\n",
		format_use_statements("", private),
		format_use_statements("pub ", public),
		rest_of_the_file.input
	))
}

// ----------------------------------------------------------------------------

fn run_file(path: &Path) -> Result<(), String> {
	if path.extension() != Some(OsStr::new("rs")) {
		return Ok(());
	}
	// println!("{:?}", path);
	let contents = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
	let pretty = prettify_code(&contents)?;
	std::fs::write(path, pretty).map_err(|e| e.to_string())?;

	std::process::Command::new("cargo")
		.arg("fmt")
		.arg("--")
		.arg(path)
		.output()
		.map_err(|err| err.to_string())?;

	Ok(())
}

fn run_path(path: &str) {
	let path = Path::new(path);
	if path.is_dir() {
		for path in ignore::Walk::new(path)
			.filter_map(Result::ok)
			.filter(|entry| entry.path().extension() == Some(OsStr::new("rs")))
		{
			let path = path.path();
			if let Err(err) = run_file(path) {
				eprintln!("ERROR processing '{}': {}", path.display(), err);
			}
		}
	} else {
		if let Err(err) = run_file(path) {
			eprintln!("ERROR processing '{}': {}", path.display(), err);
		}
	}
}

fn main() {
	let args: Vec<String> = std::env::args().collect();
	if args.is_empty() || args[0] == "--help" {
		eprintln!("Usage: userp file_or_dir [file_or_dir...]");
		eprintln!("userp clean up the use:: directives in all rust files.");
		std::process::exit(1);
	}
	for path in args {
		run_path(&path);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[macro_export]
	macro_rules! collect {
	    ($($expr: expr),*) => {
	        vec![$($expr),*].into_iter().collect()
	    };
	    ($($expr: expr,)*) => {
	        vec![$($expr),*].into_iter().collect()
	    }
	}

	#[test]
	fn parse() {
		let code = r#"
			use std::collections::HashMap;

			use std::collections::{HashSet, BTreeSet};
			use {serde, combine::*};
			use itertools::Iterator;

			rest_of_the_file"#
			.trim();
		let parse_result = use_statements().parse(code);
		let (trees, rest_of_the_file) = parse_result.unwrap_or_else(|err| panic!("Failed to parse: {}", err));
		assert_eq!(
			trees,
			vec![
				UseStatement {
					public: false,
					tree: Tree::Word(
						"std".to_string(),
						Some(Box::new(Tree::Word(
							"collections".to_string(),
							Some(Box::new(Tree::Word("HashMap".to_string(), None)))
						)))
					)
				},
				UseStatement {
					public: false,
					tree: Tree::Word(
						"std".to_string(),
						Some(Box::new(Tree::Word(
							"collections".to_string(),
							Some(Box::new(Tree::Braces(vec![
								Tree::Word("HashSet".to_string(), None),
								Tree::Word("BTreeSet".to_string(), None),
							]))),
						)))
					)
				},
				UseStatement {
					public: false,
					tree: Tree::Braces(vec![
						Tree::Word("serde".to_string(), None),
						Tree::Word("combine".to_string(), Some(Box::new(Tree::Star))),
					])
				},
				UseStatement {
					public: false,
					tree: Tree::Word(
						"itertools".to_string(),
						Some(Box::new(Tree::Word("Iterator".to_string(), None)))
					)
				},
			],
		);
		assert_eq!(rest_of_the_file, "rest_of_the_file");

		let leaf = |name: &str| {
			(
				name.to_string(),
				Node(collect![("self".to_string(), Default::default())]),
			)
		};

		let (private, public) = into_node(trees);
		assert_eq!(
			private,
			Node(collect![
				(
					"combine".to_string(),
					Node(collect![("*".to_string(), Node::default())]),
				),
				leaf("serde"),
				("itertools".to_string(), Node(collect![leaf("Iterator")]),),
				(
					"std".to_string(),
					Node(collect![(
						"collections".to_string(),
						Node(collect![leaf("HashMap"), leaf("HashSet"), leaf("BTreeSet")])
					)])
				),
			])
		);
		assert!(public.0.is_empty());
	}

	#[test]
	fn prettify_noop_1() {
		let code = "rest_of_the_file";
		assert_eq!(prettify_code(code).unwrap().trim(), code);
	}

	#[test]
	fn prettify_noop_2() {
		let code = r#"
use crate::proc::functions::JsFunctions;

foo
		"#
		.trim();

		assert_eq!(prettify_code(code).unwrap().trim(), code);
	}

	#[test]
	fn prettify_self_join() {
		let in_code = "use futures::{future, future::Future, sync::mpsc};";
		let out_code = "use futures::{future::{Future, self}, sync::mpsc};";

		assert_eq!(prettify_code(in_code).unwrap().trim(), out_code);
	}

	#[test]
	fn test_prettify_simple() {
		let in_code = r#"
use under_score::number_42;

#[test]
fn foo() {}
"#;
		assert_eq!(prettify_code(in_code).unwrap().trim(), in_code.trim());
	}

	#[test]
	fn test_prettify_1() {
		let in_code = r#"
use std::collections::{HashSet, BTreeSet};
use {serde, combine::*};
use itertools::Iterator;
use crate::foo::bar;
use crate::foo::baz;
use crate::badger;
use std::collections::HashMap;


rest_of_the_file"#
			.trim();
		let expected_code = r#"
use std::collections::{BTreeSet, HashMap, HashSet};

use {combine::*, itertools::Iterator, serde};

use crate::{badger, foo::{bar, baz}};

rest_of_the_file"#
			.trim();

		assert_eq!(prettify_code(in_code).unwrap().trim(), expected_code);
	}

	#[test]
	fn test_prettify_2() {
		let in_code = r#"
use std::collections::HashMap;
use std::error::Error;
use std::fmt;

use crate::js::{walk_expr, walk_stat, JsExpr, JsStat, Symbol, SymbolFactory, Visitor};

use crate::proc::{
    functions::JsFunctions, prng::Prng, render::ProceduralJsRenderer, InstructionNode, ProcError,
};

#[derive(Debug)]
pub enum InternerError {
    Procedural(ProcError),
    InvariantFailed,
}
		"#
		.trim();
		let expected_code = r#"
use std::{collections::HashMap, error::Error, fmt};

use crate::{js::{JsExpr, JsStat, Symbol, SymbolFactory, Visitor, walk_expr, walk_stat}, proc::{InstructionNode, ProcError, functions::JsFunctions, prng::Prng, render::ProceduralJsRenderer}};

#[derive(Debug)]
pub enum InternerError {
    Procedural(ProcError),
    InvariantFailed,
}
"#
		.trim();

		assert_eq!(prettify_code(in_code).unwrap().trim(), expected_code);
	}
}

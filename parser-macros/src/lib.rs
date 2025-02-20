#![feature(proc_macro_diagnostic)]
extern crate proc_macro;

use quote::ToTokens;
use proc_macro::{Diagnostic, Level, TokenStream};
use syn::{FnArg, NestedMeta, ItemImpl, ImplItem, Attribute, ReturnType, Error};
use darling::FromMeta;
use typed_arena::Arena;
use parser_gen::*;
use common::*;

fn parse_arg(arg: &FnArg) -> Option<(String, String)> {
  match arg {
    FnArg::Receiver(_) => None,
    FnArg::Typed(pat) => Some((pat.to_token_stream().to_string(), pat.ty.to_token_stream().to_string()))
  }
}

#[derive(FromMeta)]
struct Config {
  #[darling(default)] lex: Option<String>,
  #[darling(default)] lex_path: Option<String>,
  #[darling(default)] verbose: Option<String>,
  #[darling(default)] show_fsm: Option<String>,
  #[darling(default)] show_dfa: Option<String>,
  #[darling(default)] log_token: bool,
  #[darling(default)] log_reduce: bool,
  #[darling(default)] use_unsafe: bool,
  #[darling(default)] expand: bool,
}

// part of RawGrammar
#[derive(serde::Deserialize)]
struct RawLexer<'a> {
  #[serde(borrow)]
  priority: Vec<RawPriorityRow<'a>>,
  lexical: IndexMap<std::borrow::Cow<'a, str>, &'a str>,
  #[serde(default)] lexer_field: &'a str,
  #[serde(default)] lexer_action: &'a str,
}

#[derive(FromMeta)]
struct Rule {
  rule: String,
  #[darling(default)] prec: Option<String>,
}

struct E(Error); // pretty print `Error` with location info

impl Debug for E {
  fn fmt(&self, f: &mut Formatter) -> FmtResult {
    let lc = self.0.span().start();
    write!(f, "{} at {}:{}", self.0, lc.line, lc.column)
  }
}

fn parse_attrs(attrs: &[Attribute]) -> Vec<NestedMeta> {
  attrs.iter().map(|x| NestedMeta::Meta(x.parse_meta().map_err(E).expect("failed to parse meta"))).collect()
}

fn work(attr: TokenStream, input: TokenStream, algo: PGAlgo) -> TokenStream {
  let parser = syn::parse::<ItemImpl>(input).map_err(E).expect("failed to parse impl block");
  let start = &attr.to_string();
  let parser_def = parser.self_ty.to_token_stream().to_string();

  let Config { lex, lex_path, verbose, show_fsm, show_dfa, log_token, log_reduce, use_unsafe, expand }
    = Config::from_list(&parse_attrs(&parser.attrs)).expect("failed to read attributes");
  let lex = if let Some(lex) = lex { lex } else {
    std::fs::read_to_string(lex_path.expect("attributes must contain `lex` or `lex_path`")).expect("failed to read lex")
  };
  let mut cfg = parser_gen::Config {
    verbose: verbose.as_deref(),
    show_fsm: show_fsm.as_deref(),
    show_dfa: show_dfa.as_deref(),
    log_token,
    log_reduce,
    use_unsafe,
    lang: Lang::Rs,
    on_conflict: |c| Diagnostic::new(Level::Warning, c).emit(),
    code_output: Vec::new(),
  };
  let lex = toml::from_str::<RawLexer>(&lex).expect("failed to parse lexer toml");

  let mut production = Vec::new();
  let arena = Arena::new();
  for item in &parser.items {
    if let ImplItem::Method(method) = item {
      let Rule { rule, prec } = Rule::from_list(&parse_attrs(&method.attrs)).expect("failed to parse rule");
      let rule = &*arena.alloc_str(&rule);
      let prec = prec.map(|x| &*arena.alloc_str(&x));
      let (lhs, rhs) = parse_arrow_prod(&rule).unwrap_or_else(||
        panic!("rule \"{}\" of method `{}` is not in the form of \"lhs -> rhs1 rhs2 ...\"", rule, method.sig.ident));
      let ty = match &method.sig.output {
        ReturnType::Default => "()",
        ReturnType::Type(_, ty) => arena.alloc_str(&ty.to_token_stream().to_string()),
      };
      let rhs_arg = method.sig.inputs.iter().map(parse_arg).collect::<Vec<_>>();
      let skip_self = match rhs_arg.get(0) { Some(None) => 1, _ => 0, };
      let rhs_arg = Some(rhs_arg.into_iter().skip(skip_self).map(|arg| match arg {
        None => panic!("method `{}` takes `self` at illegal position", method.sig.ident),
        Some((pat, name)) => (&*arena.alloc_str(&pat), &*arena.alloc_str(&name)),
      }).collect());
      let act = arena.alloc_str(&method.block.to_token_stream().to_string());
      production.push(RawProduction { lhs, ty, rhs: vec![RawProductionRhs { rhs, rhs_arg, act, prec }] });
    } else { panic!("only support method impl, found {:?}", item); }
  }

  let g = RawGrammar {
    include: "",
    priority: lex.priority,
    lexical: lex.lexical,
    lexer_field: lex.lexer_field,
    lexer_action: lex.lexer_action,
    parser_field: "",
    start,
    production,
    parser_def: Some(&parser_def),
    epilogue: Some(""),
  };
  parser_gen::work(g, algo, &mut cfg).expect("failed to generate code");
  let code = unsafe { String::from_utf8_unchecked(cfg.code_output) }; // must be valid utf-8
  if expand { println!("{}", code); }
  code.parse().unwrap()
}

#[proc_macro_attribute]
pub fn lalr1(attr: TokenStream, input: TokenStream) -> TokenStream { work(attr, input, PGAlgo::LALR1) }

#[proc_macro_attribute]
pub fn ll1(attr: TokenStream, input: TokenStream) -> TokenStream { work(attr, input, PGAlgo::LL1) }
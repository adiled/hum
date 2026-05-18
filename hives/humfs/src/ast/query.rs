//! Per-language symbol queries.
//!
//! Convention used by [`ast::file_symbols`]:
//!
//! - Each captured definition pairs a `@<tag>.def` capture (whose
//!   byte range becomes the symbol's range) with a `@<tag>.name`
//!   capture (whose text becomes the symbol's name). The `.def`
//!   tag (`fn`, `method`, `class`, `const`, `type`, `enum`, `mod`)
//!   maps to `SymbolKind` via [`SymbolKind::from_tag`].
//!
//! Queries deliberately stay shallow — top-level + one nesting
//! level (methods inside classes/impls). Sub-symbol walks
//! (`body`/`when`/`otherwise`/…) live in a different module (P6).

use crate::ast::LangSpec;

pub fn symbol_query(lang: LangSpec) -> &'static str {
    match lang {
        LangSpec::Rust       => RUST_QUERY,
        LangSpec::Python     => PYTHON_QUERY,
        LangSpec::Go         => GO_QUERY,
        LangSpec::JavaScript => JS_QUERY,
        LangSpec::TypeScript => TS_QUERY,
        LangSpec::Tsx        => TS_QUERY, // TSX reuses TS surface
    }
}

const RUST_QUERY: &str = r#"
(function_item
  name: (identifier) @fn.name) @fn.def

(struct_item
  name: (type_identifier) @class.name) @class.def

(enum_item
  name: (type_identifier) @enum.name) @enum.def

(trait_item
  name: (type_identifier) @class.name) @class.def

(impl_item
  type: (type_identifier) @class.name) @class.def

(type_item
  name: (type_identifier) @type.name) @type.def

(mod_item
  name: (identifier) @mod.name) @mod.def

(const_item
  name: (identifier) @const.name) @const.def

(static_item
  name: (identifier) @const.name) @const.def
"#;

const PYTHON_QUERY: &str = r#"
(function_definition
  name: (identifier) @fn.name) @fn.def

(class_definition
  name: (identifier) @class.name) @class.def
"#;

const GO_QUERY: &str = r#"
(function_declaration
  name: (identifier) @fn.name) @fn.def

(method_declaration
  name: (field_identifier) @method.name) @method.def

(type_declaration
  (type_spec
    name: (type_identifier) @type.name)) @type.def

(const_declaration
  (const_spec
    name: (identifier) @const.name)) @const.def

(var_declaration
  (var_spec
    name: (identifier) @var.name)) @var.def
"#;

const JS_QUERY: &str = r#"
(function_declaration
  name: (identifier) @fn.name) @fn.def

(class_declaration
  name: (identifier) @class.name) @class.def

(method_definition
  name: (property_identifier) @method.name) @method.def

(lexical_declaration
  (variable_declarator
    name: (identifier) @const.name)) @const.def

(variable_declaration
  (variable_declarator
    name: (identifier) @var.name)) @var.def
"#;

const TS_QUERY: &str = r#"
(function_declaration
  name: (identifier) @fn.name) @fn.def

(class_declaration
  name: (type_identifier) @class.name) @class.def

(interface_declaration
  name: (type_identifier) @class.name) @class.def

(method_definition
  name: (property_identifier) @method.name) @method.def

(method_signature
  name: (property_identifier) @method.name) @method.def

(type_alias_declaration
  name: (type_identifier) @type.name) @type.def

(enum_declaration
  name: (identifier) @enum.name) @enum.def

(lexical_declaration
  (variable_declarator
    name: (identifier) @const.name)) @const.def

(variable_declaration
  (variable_declarator
    name: (identifier) @var.name)) @var.def
"#;

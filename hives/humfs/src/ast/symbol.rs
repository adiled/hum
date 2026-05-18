//! Symbol data model. One Symbol per top-level or nested definition
//! captured by a language's symbol query.

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// Byte range over the source. Half-open `[start_byte, end_byte)`.
    pub start_byte: usize,
    pub end_byte: usize,
    /// 1-based row range, inclusive on both ends.
    pub start_row: usize,
    pub end_row: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Method,
    Class,   // class / struct / impl / trait / interface
    Const,
    Var,     // mutable top-level binding (let, var)
    Type,    // type alias
    Enum,
    Module,  // mod / namespace
    Imports, // synthetic 'imports' block (P5)
    Other,
}

impl SymbolKind {
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "fn" | "function" => SymbolKind::Function,
            "method" => SymbolKind::Method,
            "class" | "struct" | "trait" | "interface" | "impl" => SymbolKind::Class,
            "const" => SymbolKind::Const,
            "var" | "let" => SymbolKind::Var,
            "type" => SymbolKind::Type,
            "enum" => SymbolKind::Enum,
            "mod" | "module" | "namespace" => SymbolKind::Module,
            "imports" => SymbolKind::Imports,
            _ => SymbolKind::Other,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            SymbolKind::Function => "fn",
            SymbolKind::Method   => "method",
            SymbolKind::Class    => "class",
            SymbolKind::Const    => "const",
            SymbolKind::Var      => "var",
            SymbolKind::Type     => "type",
            SymbolKind::Enum     => "enum",
            SymbolKind::Module   => "mod",
            SymbolKind::Imports  => "imports",
            SymbolKind::Other    => "?",
        }
    }
}

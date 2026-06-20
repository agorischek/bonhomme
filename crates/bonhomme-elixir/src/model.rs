#[derive(Clone, Debug, Default)]
pub(crate) struct ParsedProject {
    pub(crate) files: Vec<ParsedFile>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ParsedFile {
    pub(crate) path: String,
    pub(crate) preamble: String,
    pub(crate) declarations: Vec<Declaration>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Declaration {
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) signature: Option<String>,
    pub(crate) preamble: Option<String>,
    pub(crate) functions: Vec<ElixirFunction>,
    pub(crate) modules: Vec<Declaration>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ElixirFunction {
    pub(crate) function_name: String,
    pub(crate) arity: usize,
    pub(crate) visibility: String,
    pub(crate) kind: String,
    pub(crate) source: String,
    pub(crate) calls: Vec<CallTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CallTarget {
    Local {
        name: String,
        arity: usize,
    },
    Remote {
        module: String,
        name: String,
        arity: usize,
    },
}

impl ElixirFunction {
    pub(crate) fn symbol_name(&self) -> String {
        format!("{}/{}", self.function_name, self.arity)
    }
}

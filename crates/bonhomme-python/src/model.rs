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
    pub(crate) body: Option<String>,
    pub(crate) declaration: Option<String>,
    pub(crate) preamble: Option<String>,
    pub(crate) methods: Vec<PythonMethod>,
    pub(crate) attributes: Vec<Member>,
    pub(crate) calls: Vec<CallTarget>,
}

#[derive(Clone, Debug)]
pub(crate) struct PythonMethod {
    pub(crate) name: String,
    pub(crate) signature: String,
    pub(crate) body: String,
    pub(crate) calls: Vec<CallTarget>,
}

#[derive(Clone, Debug)]
pub(crate) struct Member {
    pub(crate) name: String,
    pub(crate) declaration: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CallTarget {
    Free(String),
    This(String),
    Method(String),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ParsedProject {
    pub(crate) files: Vec<ParsedFile>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ParsedFile {
    pub(crate) path: String,
    pub(crate) preamble: String,
    pub(crate) namespace: Option<String>,
    pub(crate) namespace_style: Option<String>,
    pub(crate) declarations: Vec<TypeDeclaration>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TypeDeclaration {
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) signature: String,
    pub(crate) body_preamble: String,
    pub(crate) members: Vec<MemberDeclaration>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MemberDeclaration {
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) signature: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) declaration: Option<String>,
    pub(crate) calls: Vec<CallTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CallTarget {
    Free(String),
    This(String),
    Method(String),
}

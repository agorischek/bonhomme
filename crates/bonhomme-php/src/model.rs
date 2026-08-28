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
    pub(crate) namespace_header: Option<String>,
    pub(crate) namespace_body_preamble: String,
    pub(crate) imports: Vec<String>,
    pub(crate) declarations: Vec<FileDeclaration>,
}

#[derive(Clone, Debug)]
pub(crate) enum FileDeclaration {
    Type(TypeDeclaration),
    Function(MemberDeclaration),
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
    pub(crate) identity: String,
    pub(crate) signature: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) declaration: Option<String>,
    pub(crate) calls: Vec<CallTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CallTarget {
    Function(String),
    This(String),
    Method(String),
    Constructor(String),
}

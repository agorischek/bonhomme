#[derive(Clone, Debug, Default)]
pub(crate) struct ParsedCrate {
    pub(crate) files: Vec<ParsedFile>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ParsedFile {
    pub(crate) path: String,
    pub(crate) uses: Vec<String>,
    pub(crate) declarations: Vec<Declaration>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Declaration {
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) graph_name: String,
    pub(crate) declaration: String,
    pub(crate) signature: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) fields: Vec<Member>,
    pub(crate) variants: Vec<Member>,
    pub(crate) trait_methods: Vec<RustMethod>,
    pub(crate) methods: Vec<RustMethod>,
    pub(crate) impl_type: Option<String>,
    pub(crate) impl_header: Option<String>,
    pub(crate) calls: Vec<CallTarget>,
}

#[derive(Clone, Debug)]
pub(crate) struct Member {
    pub(crate) name: String,
    pub(crate) declaration: String,
}

#[derive(Clone, Debug)]
pub(crate) struct RustMethod {
    pub(crate) name: String,
    pub(crate) signature: String,
    pub(crate) body: Option<String>,
    pub(crate) impl_type: Option<String>,
    pub(crate) impl_header: Option<String>,
    pub(crate) calls: Vec<CallTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CallTarget {
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) receiver: Option<String>,
}

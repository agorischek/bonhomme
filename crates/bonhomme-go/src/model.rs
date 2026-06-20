use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ParseRequest<'a> {
    pub(crate) files: &'a [bonhomme_core::RenderedFile],
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ParsedPackage {
    pub(crate) files: Vec<ParsedFile>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ParsedFile {
    pub(crate) path: String,
    pub(crate) package_name: String,
    pub(crate) imports: String,
    pub(crate) declarations: Vec<Declaration>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Declaration {
    pub(crate) kind: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) receiver: Option<String>,
    #[serde(default)]
    pub(crate) signature: Option<String>,
    #[serde(default)]
    pub(crate) body: Option<String>,
    #[serde(default)]
    pub(crate) declaration: Option<String>,
    #[serde(default)]
    pub(crate) fields: Vec<Field>,
    #[serde(default)]
    pub(crate) methods: Vec<InterfaceMethod>,
    #[serde(default)]
    pub(crate) calls: Vec<CallTarget>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) declaration: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InterfaceMethod {
    pub(crate) name: String,
    pub(crate) signature: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CallTarget {
    pub(crate) kind: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) receiver: Option<String>,
}

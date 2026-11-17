use crate::{
    error::CoreError,
    model::{AuthConfig, EffectiveAuth, FolderNode, HttpRequestDoc, ProjectNode},
};

/// Identifies a node in the authentication inheritance hierarchy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthNodeRef {
    Project { id: String, name: String },
    Folder { id: String, name: String },
    Request { id: String, name: String },
}

impl AuthNodeRef {
    pub fn id(&self) -> &str {
        match self {
            Self::Project { id, .. } | Self::Folder { id, .. } | Self::Request { id, .. } => id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Project { name, .. } | Self::Folder { name, .. } | Self::Request { name, .. } => {
                name
            }
        }
    }

    pub const fn kind_label(&self) -> &'static str {
        match self {
            Self::Project { .. } => "Project",
            Self::Folder { .. } => "Folder",
            Self::Request { .. } => "Request",
        }
    }
}

/// One node evaluated along the inheritance chain (from target up to Project).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthChainStep {
    pub node: AuthNodeRef,
    pub configured: AuthConfig,
}

/// Effective authentication resolved for a node plus its full inheritance trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAuth {
    pub effective: EffectiveAuth,
    pub source: Option<AuthNodeRef>,
    /// Ordered from the evaluated target node up to the Project root.
    pub chain: Vec<AuthChainStep>,
}

impl ResolvedAuth {
    /// Returns true when the effective auth came from a parent folder or project.
    pub fn is_inherited(&self) -> bool {
        matches!(
            (self.chain.first(), self.source.as_ref()),
            (Some(first), Some(source)) if first.node != *source
        )
    }

    /// Short human-readable summary suitable for toolbars and breadcrumbs.
    pub fn summary(&self) -> String {
        match (&self.effective, &self.source) {
            (EffectiveAuth::None, Some(src)) if self.is_inherited() => {
                format!(
                    "No Auth (inherited from {} \"{}\")",
                    src.kind_label(),
                    src.name()
                )
            }
            (EffectiveAuth::None, _) => "No Auth".to_string(),
            (auth, Some(src)) if self.is_inherited() => {
                format!(
                    "{} (from {} \"{}\")",
                    auth.mode_label(),
                    src.kind_label(),
                    src.name()
                )
            }
            (auth, _) => auth.mode_label().to_string(),
        }
    }
}

/// Resolves authentication for a request stored in `project`.
pub fn resolve_request_auth(
    project: &ProjectNode,
    request_id: &str,
) -> Result<ResolvedAuth, CoreError> {
    let (ancestors, req) =
        project
            .find_request(request_id)
            .ok_or_else(|| CoreError::RequestNotFound {
                request_id: request_id.to_string(),
            })?;
    Ok(resolve_request_doc_auth(project, &ancestors, &req.doc))
}

/// Resolves authentication for a potentially unsaved [`HttpRequestDoc`] located under `ancestors`.
pub fn resolve_request_doc_auth(
    project: &ProjectNode,
    ancestors: &[&FolderNode],
    doc: &HttpRequestDoc,
) -> ResolvedAuth {
    let mut chain = Vec::with_capacity(ancestors.len() + 2);
    chain.push(AuthChainStep {
        node: AuthNodeRef::Request {
            id: doc.id.clone(),
            name: doc.name.clone(),
        },
        configured: doc.request.auth.clone(),
    });

    for folder in ancestors.iter().rev() {
        chain.push(AuthChainStep {
            node: AuthNodeRef::Folder {
                id: folder.doc.id.clone(),
                name: folder.doc.name.clone(),
            },
            configured: folder.doc.auth.clone(),
        });
    }

    chain.push(AuthChainStep {
        node: AuthNodeRef::Project {
            id: project.doc.id.clone(),
            name: project.doc.name.clone(),
        },
        configured: project.doc.auth.clone(),
    });

    resolve_chain(chain)
}

/// Resolves authentication for a folder inside `project`.
pub fn resolve_folder_auth(
    project: &ProjectNode,
    folder_id: &str,
) -> Result<ResolvedAuth, CoreError> {
    let ancestors = project
        .find_folder(folder_id)
        .ok_or_else(|| CoreError::FolderNotFound {
            folder_id: folder_id.to_string(),
        })?;
    Ok(resolve_folder_chain_auth(project, &ancestors))
}

/// Resolves authentication for a folder chain (outermost to target) inside `project`.
pub fn resolve_folder_chain_auth(
    project: &ProjectNode,
    folder_chain: &[&FolderNode],
) -> ResolvedAuth {
    let mut chain = Vec::with_capacity(folder_chain.len() + 1);
    for folder in folder_chain.iter().rev() {
        chain.push(AuthChainStep {
            node: AuthNodeRef::Folder {
                id: folder.doc.id.clone(),
                name: folder.doc.name.clone(),
            },
            configured: folder.doc.auth.clone(),
        });
    }
    chain.push(AuthChainStep {
        node: AuthNodeRef::Project {
            id: project.doc.id.clone(),
            name: project.doc.name.clone(),
        },
        configured: project.doc.auth.clone(),
    });
    resolve_chain(chain)
}

/// Resolves authentication at the project root.
pub fn resolve_project_auth(project: &ProjectNode) -> ResolvedAuth {
    let chain = vec![AuthChainStep {
        node: AuthNodeRef::Project {
            id: project.doc.id.clone(),
            name: project.doc.name.clone(),
        },
        configured: project.doc.auth.clone(),
    }];
    resolve_chain(chain)
}

fn resolve_chain(chain: Vec<AuthChainStep>) -> ResolvedAuth {
    for step in &chain {
        if !matches!(step.configured, AuthConfig::Inherit) {
            return ResolvedAuth {
                effective: step.configured.into_effective(),
                source: Some(step.node.clone()),
                chain,
            };
        }
    }

    ResolvedAuth {
        effective: EffectiveAuth::None,
        source: chain.last().map(|s| s.node.clone()),
        chain,
    }
}

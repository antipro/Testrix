use crate::model::{FolderNode, ProjectNode, RequestNode, WorkspaceTree};

/// Filtered view of a project for the explorer tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilteredProject<'a> {
    pub project: &'a ProjectNode,
    pub folders: Vec<FilteredFolder<'a>>,
    pub requests: Vec<&'a RequestNode>,
}

/// Filtered view of a folder preserving ancestor chain when descendants match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilteredFolder<'a> {
    pub folder: &'a FolderNode,
    pub folders: Vec<FilteredFolder<'a>>,
    pub requests: Vec<&'a RequestNode>,
}

/// Filters all projects, folders, and HTTP requests in `workspace` using `query`.
///
/// An empty query returns the complete workspace hierarchy unmodified.
pub fn filter_workspace<'a>(workspace: &'a WorkspaceTree, query: &str) -> Vec<FilteredProject<'a>> {
    let needle = query.trim().to_ascii_lowercase();
    workspace
        .projects
        .iter()
        .filter_map(|project| filter_project(project, &needle))
        .collect()
}

fn filter_project<'a>(project: &'a ProjectNode, needle: &str) -> Option<FilteredProject<'a>> {
    if needle.is_empty() || project.doc.name.to_ascii_lowercase().contains(needle) {
        return Some(FilteredProject {
            project,
            folders: project.folders.iter().map(unfiltered_folder).collect(),
            requests: project.requests.iter().collect(),
        });
    }

    let folders: Vec<FilteredFolder<'a>> = project
        .folders
        .iter()
        .filter_map(|folder| filter_folder(folder, needle))
        .collect();
    let requests: Vec<&'a RequestNode> = project
        .requests
        .iter()
        .filter(|req| request_matches(req, needle))
        .collect();

    if folders.is_empty() && requests.is_empty() {
        None
    } else {
        Some(FilteredProject {
            project,
            folders,
            requests,
        })
    }
}

fn filter_folder<'a>(folder: &'a FolderNode, needle: &str) -> Option<FilteredFolder<'a>> {
    if folder.doc.name.to_ascii_lowercase().contains(needle) {
        return Some(unfiltered_folder(folder));
    }

    let folders: Vec<FilteredFolder<'a>> = folder
        .folders
        .iter()
        .filter_map(|sub| filter_folder(sub, needle))
        .collect();
    let requests: Vec<&'a RequestNode> = folder
        .requests
        .iter()
        .filter(|req| request_matches(req, needle))
        .collect();

    if folders.is_empty() && requests.is_empty() {
        None
    } else {
        Some(FilteredFolder {
            folder,
            folders,
            requests,
        })
    }
}

fn unfiltered_folder<'a>(folder: &'a FolderNode) -> FilteredFolder<'a> {
    FilteredFolder {
        folder,
        folders: folder.folders.iter().map(unfiltered_folder).collect(),
        requests: folder.requests.iter().collect(),
    }
}

fn request_matches(req: &RequestNode, needle: &str) -> bool {
    req.doc.name.to_ascii_lowercase().contains(needle)
        || req.doc.request.url.to_ascii_lowercase().contains(needle)
        || req
            .doc
            .request
            .method
            .as_str()
            .to_ascii_lowercase()
            .contains(needle)
}

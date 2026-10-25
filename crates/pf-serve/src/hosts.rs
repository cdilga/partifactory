//! Host-based routing (§3 Hostnames): `parts.<apex>` is the workspace, `<name>-part.<apex>` a
//! part host, `blobs-part.<apex>` the shared blob host; anything else on the wildcard is a
//! non-part host (404, and counted in `pf_nonpart_host_hits_total`).

/// What a request's `Host` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostClass {
    Workspace,
    /// A part host; the value is the part name (the label without the suffix).
    Part(String),
    Blobs,
    /// On the wildcard but not a part host, or no usable `Host` at all.
    Other,
}

impl HostClass {
    /// The `host_class` log field (never the name itself).
    pub fn as_str(&self) -> &'static str {
        match self {
            HostClass::Workspace => "workspace",
            HostClass::Part(_) => "part",
            HostClass::Blobs => "blobs",
            HostClass::Other => "other",
        }
    }
}

/// The part host of a request, inserted as a request extension on the content listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartHost(pub String);

#[derive(Debug, Clone)]
pub struct HostRules {
    apex: String,
    workspace: String,
    suffix: String,
}

impl HostRules {
    pub fn new(apex: &str, workspace_host: &str, part_suffix: &str) -> Self {
        Self {
            apex: apex.trim_end_matches('.').to_ascii_lowercase(),
            workspace: workspace_host.to_ascii_lowercase(),
            suffix: part_suffix.to_ascii_lowercase(),
        }
    }

    /// The apex beneath which workspace and part hosts are classified.
    pub fn apex(&self) -> &str {
        &self.apex
    }

    /// The workspace's full host name, e.g. `parts.pf.test`.
    pub fn workspace_host(&self) -> String {
        format!("{}.{}", self.workspace, self.apex)
    }

    /// A part's full host name, e.g. `notes-part.pf.test`.
    pub fn part_host(&self, name: &str) -> String {
        format!("{name}{}.{}", self.suffix, self.apex)
    }

    /// The blob host, e.g. `blobs-part.pf.test`.
    pub fn blob_host(&self) -> String {
        self.part_host("blobs")
    }

    /// Classify a `Host` value (port and trailing dot ignored, case-insensitive).
    pub fn classify(&self, host: &str) -> HostClass {
        let host = strip_port(host).trim_end_matches('.').to_ascii_lowercase();
        if self.apex.is_empty() || host.is_empty() {
            return HostClass::Other;
        }
        let Some(label) = host
            .strip_suffix(&self.apex)
            .and_then(|l| l.strip_suffix('.'))
        else {
            return HostClass::Other;
        };
        if label.is_empty() || label.contains('.') {
            return HostClass::Other;
        }
        if label == self.workspace {
            return HostClass::Workspace;
        }
        match label.strip_suffix(&self.suffix) {
            Some("blobs") => HostClass::Blobs,
            Some(name) if is_part_name(name) => HostClass::Part(name.to_string()),
            _ => HostClass::Other,
        }
    }
}

fn strip_port(host: &str) -> &str {
    if host.starts_with('[') {
        return host; // an IPv6 literal is never one of ours
    }
    match host.rsplit_once(':') {
        Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    }
}

/// A part name as it can appear in a host label: lowercase letters, digits and inner hyphens.
pub fn is_part_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_hosts() {
        let r = HostRules::new("pf.test", "parts", "-part");
        assert_eq!(r.classify("parts.pf.test"), HostClass::Workspace);
        assert_eq!(r.classify("PARTS.pf.test:443"), HostClass::Workspace);
        assert_eq!(
            r.classify("notes-part.pf.test"),
            HostClass::Part("notes".into())
        );
        assert_eq!(
            r.classify("my-notes-part.pf.test."),
            HostClass::Part("my-notes".into())
        );
        assert_eq!(r.classify("blobs-part.pf.test"), HostClass::Blobs);
        assert_eq!(r.classify("forge.pf.test"), HostClass::Other);
        assert_eq!(r.classify("-part.pf.test"), HostClass::Other);
        assert_eq!(r.classify("a.b-part.pf.test"), HostClass::Other);
        assert_eq!(r.classify("notes-part.evil.test"), HostClass::Other);
        assert_eq!(r.classify("pf.test"), HostClass::Other);
        assert_eq!(r.classify(""), HostClass::Other);
        assert_eq!(r.workspace_host(), "parts.pf.test");
        assert_eq!(r.blob_host(), "blobs-part.pf.test");
    }
}

use crate::{
    context::Context,
    render::{Layout, Plan, Regular, Rows, theme},
    styles,
    tree::Tree,
};

impl Layout for Regular {
    /// The tree upside down: nodes in reverse pre-order, root last.
    fn rows(tree: &Tree, ctx: &Context) -> Rows {
        if ctx.follow {
            Rows::Prefixed(plan(tree, ctx))
        } else {
            Rows::Tree {
                reverse: true,
                glyphs: theme::GlyphCache::new().prefix_glyphs(false),
            }
        }
    }
}

/// The rows in order with their prefixes, for `--follow`, where the theme of a row depends on
/// the rows before it. This replays the edges of a reverse depth-first traversal: before a
/// leaf, the `End` edges of the directories whose subtree it closes (outermost first), then the
/// `Start` edge of each node.
fn plan(tree: &Tree, ctx: &Context) -> Plan {
    let root = tree.root();
    let max_depth = ctx.level();
    let order = tree.order();
    let mut plan = Plan::with_capacity(order.len());

    let glyphs = theme::GlyphCache::new();
    let mut get_theme = theme::link_theme_getter();

    let mut base: Vec<&str> = vec![];
    let mut closing = vec![];

    for (k, &id) in order.iter().enumerate().rev() {
        if !tree.has_children(id) {
            // Directories whose last pre-order descendant is this leaf, innermost first.
            closing.clear();
            let mut cur = id;
            while cur != root && tree.is_last(cur) {
                let parent = tree.parent(cur);
                if parent == root {
                    break;
                }
                closing.push(parent);
                cur = parent;
            }

            for &dir in closing.iter().rev() {
                let node = tree.node(dir);
                if !node.is_dir() {
                    continue;
                }

                let theme = glyphs.get(get_theme(node));

                if tree.is_last(dir) {
                    base.push(styles::SEP);
                } else {
                    base.push(theme.vt);
                }
            }
        }

        let node = tree.node(id);
        let depth = node.depth();
        let theme = glyphs.get(get_theme(node));

        if depth <= max_depth {
            if depth == 0 {
                plan.push_with_prefix(id, [""]);
            } else {
                let part = if tree.is_last(id) {
                    theme.drt
                } else {
                    theme.vtrt
                };

                plan.push_with_prefix(id, base.iter().copied().chain([part]));
            }
        }

        // The next edge is the parent's `Start` exactly when this node is its first child.
        if let Some(&prev) = k.checked_sub(1).and_then(|p| order.get(p))
            && tree.has_children(prev)
            && tree.node(prev).depth() < depth
        {
            base.pop();
        }
    }

    plan
}

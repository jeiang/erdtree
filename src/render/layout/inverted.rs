use crate::{
    context::Context,
    render::{Inverted, Layout, Plan, Rows, theme},
    styles,
    tree::Tree,
};

impl Layout for Inverted {
    /// The tree with the root at the top, nodes in pre-order.
    fn rows(tree: &Tree, ctx: &Context) -> Rows {
        if ctx.follow {
            Rows::Prefixed(plan(tree, ctx))
        } else {
            Rows::Tree {
                reverse: false,
                glyphs: theme::prefix_glyphs(true),
            }
        }
    }
}

/// The rows in order with their prefixes, for `--follow`, where the theme of a row depends on
/// the rows before it.
fn plan(tree: &Tree, ctx: &Context) -> Plan {
    let level = ctx.level();
    let order = tree.order();
    let mut plan = Plan::with_capacity(order.len());

    plan.push_with_prefix(tree.root(), [""]);

    let branches = styles::theme();
    let mut links = theme::LinkTracker::new();

    // Box-drawing segments of the ancestors below the root, outermost first.
    let mut base: Vec<&str> = vec![];

    for (k, &id) in order.iter().enumerate().skip(1) {
        let node = tree.node(id);
        let depth = node.depth();
        let last_sibling = tree.is_last(id);
        let glyphs = branches.glyphs(links.is_link(node));

        if depth <= level {
            let part: &str = if last_sibling {
                &glyphs.uprt
            } else {
                &glyphs.vtrt
            };

            plan.push_with_prefix(id, base.iter().copied().chain([part]));
        }

        if let Some(&next) = order.get(k + 1) {
            let next_depth = tree.node(next).depth();

            if next_depth == depth + 1 {
                if last_sibling {
                    base.push(styles::SEP);
                } else {
                    base.push(&glyphs.vt);
                }
            } else if next_depth < depth {
                base.truncate(base.len() - (depth - next_depth));
            }
        }
    }

    plan
}

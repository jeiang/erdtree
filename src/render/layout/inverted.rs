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
            Rows::Plan(plan(tree, ctx))
        } else {
            Rows::Tree {
                reverse: false,
                glyphs: theme::GlyphCache::new().prefix_glyphs(true),
            }
        }
    }
}

/// The rows in order with their prefixes, for `--follow`, where the theme of a row depends on
/// the rows before it.
fn plan(tree: &Tree, ctx: &Context) -> Plan {
    let level = ctx.level();
    let order = tree.order();
    let mut plan = Plan::with_capacity(order.len(), true);

    plan.push_with_prefix(tree.root(), [""]);

    let glyphs = theme::GlyphCache::new();
    let mut get_theme = theme::link_theme_getter();

    // Box-drawing segments of the ancestors below the root, outermost first.
    let mut base: Vec<&str> = vec![];

    for (k, &id) in order.iter().enumerate().skip(1) {
        let node = tree.node(id);
        let depth = node.depth();
        let last_sibling = tree.is_last(id);
        let theme = glyphs.get(get_theme(node));

        if depth <= level {
            let part = if last_sibling { theme.uprt } else { theme.vtrt };

            plan.push_with_prefix(id, base.iter().copied().chain([part]));
        }

        if let Some(&next) = order.get(k + 1) {
            let next_depth = tree.node(next).depth();

            if next_depth == depth + 1 {
                if last_sibling {
                    base.push(styles::SEP);
                } else {
                    base.push(theme.vt);
                }
            } else if next_depth < depth {
                base.truncate(base.len() - (depth - next_depth));
            }
        }
    }

    plan
}

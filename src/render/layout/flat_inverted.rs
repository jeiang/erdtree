use crate::{
    context::Context,
    render::{FlatInverted, Layout, Plan, Rows},
    tree::Tree,
};

impl Layout for FlatInverted {
    /// The flat order, root first.
    fn rows(tree: &Tree, ctx: &Context) -> Rows {
        Rows::Plan(Plan::flat(
            tree.flat_order(ctx),
            tree,
            ctx.level(),
            ctx.threads,
        ))
    }
}

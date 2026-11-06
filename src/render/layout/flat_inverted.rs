use crate::{
    context::Context,
    render::{FlatInverted, Layout, Rows},
    tree::Tree,
};

impl Layout for FlatInverted {
    /// The flat order, root first.
    fn rows(tree: &Tree, ctx: &Context) -> Rows {
        Rows::flat(tree.flat_order(ctx), tree, ctx.level(), ctx.threads)
    }
}

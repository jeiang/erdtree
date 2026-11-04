use crate::{
    context::Context,
    render::{Flat, Layout, Plan, Rows},
    tree::Tree,
};

impl Layout for Flat {
    /// The flat order reversed, root last.
    fn rows(tree: &Tree, ctx: &Context) -> Rows {
        let mut ids = tree.flat_order(ctx);
        ids.reverse();

        Rows::Plan(Plan::flat(ids, tree, ctx.level(), ctx.threads))
    }
}

use crate::{
    context::Context,
    render::{Flat, Layout, Plan, Rows},
    tree::Tree,
};

impl Layout for Flat {
    /// The flat order reversed, root last.
    fn rows(tree: &Tree, ctx: &Context) -> Rows {
        let max_depth = ctx.level();
        let mut plan = Plan::with_capacity(tree.order().len(), false);

        for id in tree.flat_order(ctx).into_iter().rev() {
            if tree.node(id).depth() <= max_depth {
                plan.push(id);
            }
        }

        Rows::Plan(plan.with_paths(tree, max_depth, ctx.threads))
    }
}

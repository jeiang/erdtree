use crate::{
    context::Context,
    render::{FlatInverted, Layout, Plan, Rows},
    tree::Tree,
};

impl Layout for FlatInverted {
    /// The flat order, root first.
    fn rows(tree: &Tree, ctx: &Context) -> Rows {
        let max_depth = ctx.level();
        let mut plan = Plan::with_capacity(tree.order().len(), false);

        for id in tree.flat_order(ctx) {
            if tree.node(id).depth() <= max_depth {
                plan.push(id);
            }
        }

        Rows::Plan(plan)
    }
}

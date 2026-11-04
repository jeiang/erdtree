use crate::{
    context::Context,
    render::{Flat, Layout, Plan, grid},
    tree::Tree,
};

impl Layout for Flat {
    type Grid = grid::Flat;

    /// The flat order reversed, root last.
    fn plan(tree: &Tree, ctx: &Context) -> Plan {
        let max_depth = ctx.level();
        let mut plan = Plan::with_capacity(tree.order().len(), false);

        for id in tree.flat_order(ctx).into_iter().rev() {
            if tree.node(id).depth() <= max_depth {
                plan.push(id);
            }
        }

        plan
    }
}

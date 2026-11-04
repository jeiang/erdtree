use crate::{
    context::Context,
    render::{FlatInverted, Layout, Plan, grid},
    tree::Tree,
};

impl Layout for FlatInverted {
    type Grid = grid::Flat;

    /// The flat order, root first.
    fn plan(tree: &Tree, ctx: &Context) -> Plan {
        let max_depth = ctx.level();
        let mut plan = Plan::with_capacity(tree.order().len(), false);

        for id in tree.flat_order(ctx) {
            if tree.node(id).depth() <= max_depth {
                plan.push(id);
            }
        }

        plan
    }
}

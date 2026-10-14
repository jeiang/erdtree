use crate::render::{
    Engine, FlatInverted,
    grid::{self, Row},
};
use std::fmt::{self, Display};

impl Display for Engine<FlatInverted> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ctx = self.context();
        let tree = self.tree();
        let arena = tree.arena();
        let order = tree.flat_order(ctx);
        let max_depth = ctx.level();

        for node_id in order {
            let node = arena[node_id].get();

            if node.depth() > max_depth {
                continue;
            }

            let row = Row::<grid::Flat>::new(node, ctx, None);

            writeln!(f, "{row}")?;
        }

        Ok(())
    }
}

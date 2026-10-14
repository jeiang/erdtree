use crate::{
    render::{
        Engine, Flat,
        grid::{self, Row},
    },
    tree::{Tree, count::FileCount},
};
use std::fmt::{self, Display};

impl Display for Engine<Flat> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ctx = self.context();
        let tree = self.tree();
        let arena = tree.arena();
        let order = tree.flat_order(ctx);
        let max_depth = ctx.level();
        let mut file_count_data = vec![];

        for node_id in order.into_iter().rev() {
            file_count_data.push(Tree::compute_file_count(node_id, arena));

            let node = arena[node_id].get();

            if node.depth() > max_depth {
                continue;
            }

            let row = Row::<grid::Flat>::new(node, ctx, None);

            writeln!(f, "{row}")?;
        }

        if !ctx.no_report && !file_count_data.is_empty() {
            write!(
                f,
                "\n{}",
                FileCount::from(file_count_data) + tree.hidden_count()
            )?;
        }

        Ok(())
    }
}

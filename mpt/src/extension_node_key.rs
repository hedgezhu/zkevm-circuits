use halo2::{
    circuit::Chip,
    plonk::{
        Advice, Column, ConstraintSystem, Expression, Fixed, VirtualCells,
    },
    poly::Rotation,
};
use itertools::Itertools;
use pairing::arithmetic::FieldExt;
use std::marker::PhantomData;

use crate::{
    helpers::compute_rlc,
    mpt::FixedTableTag,
    param::{
        HASH_WIDTH, IS_EXTENSION_EVEN_KEY_LEN_POS, IS_EXTENSION_KEY_LONG_POS,
        IS_EXTENSION_KEY_SHORT_POS, IS_EXTENSION_NODE_POS,
        IS_EXTENSION_ODD_KEY_LEN_POS, LAYOUT_OFFSET,
    },
};

#[derive(Clone, Debug)]
pub(crate) struct ExtensionNodeKeyConfig {}

pub(crate) struct ExtensionNodeKeyChip<F> {
    config: ExtensionNodeKeyConfig,
    _marker: PhantomData<F>,
}

impl<F: FieldExt> ExtensionNodeKeyChip<F> {
    pub fn configure(
        meta: &mut ConstraintSystem<F>,
        q_not_first: Column<Fixed>,
        not_first_level: Column<Fixed>, // to avoid rotating back when in the first branch (for key rlc)
        is_branch_init: Column<Advice>,
        is_branch_child: Column<Advice>,
        is_last_branch_child: Column<Advice>,
        is_account_leaf_storage_codehash_c: Column<Advice>,
        s_rlp2: Column<Advice>,
        s_advices: [Column<Advice>; HASH_WIDTH],
        modified_node: Column<Advice>, // index of the modified node
        // sel1 and sel2 in branch init: denote whether it's the first or second nibble of the key byte
        // sel1 and sel2 in branch children: denote whether there is no leaf at is_modified (when value is added or deleted from trie)
        sel1: Column<Advice>,
        sel2: Column<Advice>,
        key_rlc: Column<Advice>, // used first for account address, then for storage key
        key_rlc_mult: Column<Advice>,
        mult_diff: Column<Advice>,
        fixed_table: [Column<Fixed>; 3],
        r_table: Vec<Expression<F>>,
    ) -> ExtensionNodeKeyConfig {
        let config = ExtensionNodeKeyConfig {};
        let one = Expression::Constant(F::one());
        let c128 = Expression::Constant(F::from(128));
        let c16 = Expression::Constant(F::from(16));
        let c16inv = Expression::Constant(F::from(16).invert().unwrap());
        let rot_into_branch_init = -18;

        // TODO: s_advices 0 after key len

        meta.create_gate("extension node key", |meta| {
            let q_not_first = meta.query_fixed(q_not_first, Rotation::cur());
            let not_first_level =
                meta.query_fixed(not_first_level, Rotation::cur());

            let mut constraints = vec![];

            // Could be used any rotation into previous branch, because key RLC is the same in all
            // branch children:
            let rot_into_prev_branch = rot_into_branch_init - 3;

            let is_extension_node = meta.query_advice(
                s_advices[IS_EXTENSION_NODE_POS - LAYOUT_OFFSET],
                Rotation(rot_into_branch_init),
            );

            // NOTE: is_key_even and is_key_odd is for number of nibbles that
            // are compactly encoded.
            let is_key_even = meta.query_advice(
                s_advices[IS_EXTENSION_EVEN_KEY_LEN_POS - LAYOUT_OFFSET],
                Rotation(rot_into_branch_init),
            );
            let is_key_odd = meta.query_advice(
                s_advices[IS_EXTENSION_ODD_KEY_LEN_POS - LAYOUT_OFFSET],
                Rotation(rot_into_branch_init),
            );
            let is_short = meta.query_advice(
                s_advices[IS_EXTENSION_KEY_SHORT_POS - LAYOUT_OFFSET],
                Rotation(rot_into_branch_init),
            );
            let is_long = meta.query_advice(
                s_advices[IS_EXTENSION_KEY_LONG_POS - LAYOUT_OFFSET],
                Rotation(rot_into_branch_init),
            );

            // sel1 and sel2 determines whether branch modified_node needs to be
            // multiplied by 16 or not. However, implicitly, sel1 and sel2 determines
            // also (together with extension node key length) whether the extension
            // node key nibble needs to be multiplied by 16 or not.
            let sel1 =
                meta.query_advice(sel1, Rotation(rot_into_branch_init));
            let sel2 =
                meta.query_advice(sel2, Rotation(rot_into_branch_init));

            // We are in extension row C, -18 brings us in the branch init row.
            // -19 is account leaf storage codehash when we are in the first storage proof level.
            let is_account_leaf_storage_codehash_prev = meta.query_advice(
                is_account_leaf_storage_codehash_c,
                Rotation(rot_into_branch_init-1),
            );

            let is_branch_init_prev =
                meta.query_advice(is_branch_init, Rotation::prev());
            let is_branch_child_prev =
                meta.query_advice(is_branch_child, Rotation::prev());
            let is_branch_child_cur =
                meta.query_advice(is_branch_child, Rotation::cur());

            // Any rotation that lands into branch children can be used:
            let modified_node_cur =
                meta.query_advice(modified_node, Rotation(-2));

            let is_extension_s_row =
                meta.query_advice(is_last_branch_child, Rotation(-1));
            let is_extension_c_row =
                meta.query_advice(is_last_branch_child, Rotation(-2));

            let key_rlc_prev = meta.query_advice(key_rlc, Rotation::prev());
            let key_rlc_prev_level = meta.query_advice(key_rlc, Rotation(rot_into_prev_branch));
            let key_rlc_cur = meta.query_advice(key_rlc, Rotation::cur());

            let mult_diff_cur = meta.query_advice(mult_diff, Rotation::cur());
            let mult_diff_prev = meta.query_advice(mult_diff, Rotation::prev());

            let key_rlc_mult_prev = meta.query_advice(key_rlc_mult, Rotation::prev());
            let key_rlc_mult_prev_level = meta.query_advice(key_rlc_mult, Rotation(rot_into_prev_branch));
            let key_rlc_mult_cur = meta.query_advice(key_rlc_mult, Rotation::cur());

            // Any rotation into branch children can be used:
            let key_rlc_branch = meta.query_advice(key_rlc, Rotation(rot_into_branch_init+1));
            let key_rlc_mult_branch = meta.query_advice(key_rlc_mult, Rotation(rot_into_branch_init+1));
            let mult_diff = meta.query_advice(mult_diff, Rotation(rot_into_branch_init+1));

            constraints.push((
                "branch key RLC same over all branch children with index > 0",
                q_not_first.clone()
                    * is_branch_child_prev.clone()
                    * is_branch_child_cur.clone()
                    * (key_rlc_cur.clone() - key_rlc_prev.clone()),
            ));
            constraints.push((
                "branch key RLC MULT same over all branch children with index > 0",
                q_not_first.clone()
                    * is_branch_child_prev.clone()
                    * is_branch_child_cur.clone()
                    * (key_rlc_mult_cur.clone() - key_rlc_mult_prev.clone()),
            ));
            constraints.push((
                "branch key MULT diff same over all branch children with index > 0",
                q_not_first.clone()
                    * is_branch_child_prev.clone()
                    * is_branch_child_cur.clone()
                    * (mult_diff_cur.clone() - mult_diff_prev.clone()),
            ));

            constraints.push((
                "extension node row S key RLC is the same as branch key RLC when NOT extension node",
                q_not_first.clone()
                    * (one.clone() - is_branch_init_prev.clone()) // to prevent Poisoned Constraint due to rotation for is_extension_node
                    * (one.clone() - is_branch_child_prev.clone()) // to prevent Poisoned Constraint
                    * is_extension_s_row.clone()
                    * (one.clone() - is_extension_node.clone())
                    * (key_rlc_cur.clone() - key_rlc_prev.clone()),
            ));
            constraints.push((
                "extension node row S key RLC mult is the same as branch key RLC when NOT extension node",
                q_not_first.clone()
                    * (one.clone() - is_branch_init_prev.clone()) // to prevent Poisoned Constraint due to rotation for is_extension_node
                    * (one.clone() - is_branch_child_prev.clone()) // to prevent Poisoned Constraint
                    * is_extension_s_row.clone()
                    * (one.clone() - is_extension_node.clone())
                    * (key_rlc_mult_cur.clone() - key_rlc_mult_prev.clone()),
            ));

            constraints.push((
                "extension node row C key RLC is the same as branch key RLC when NOT extension node",
                q_not_first.clone()
                    * (one.clone() - is_branch_init_prev.clone()) // to prevent Poisoned Constraint due to rotation for is_extension_node
                    * (one.clone() - is_branch_child_prev.clone()) // to prevent Poisoned Constraint
                    * is_extension_c_row.clone()
                    * (one.clone() - is_extension_node.clone())
                    * (key_rlc_cur.clone() - key_rlc_prev.clone()),
            ));
            constraints.push((
                "extension node row C key RLC mult is the same as branch key RLC when NOT extension node",
                q_not_first.clone()
                    * (one.clone() - is_branch_init_prev.clone()) // to prevent Poisoned Constraint due to rotation for is_extension_node
                    * (one.clone() - is_branch_child_prev.clone()) // to prevent Poisoned Constraint
                    * is_extension_c_row.clone()
                    * (one.clone() - is_extension_node.clone())
                    * (key_rlc_mult_cur.clone() - key_rlc_mult_prev.clone()),
            ));

            // First level in account proof:

            let account_first = q_not_first.clone()
                    * (one.clone() - is_branch_init_prev.clone()) // to prevent Poisoned Constraint due to rotation for is_extension_node
                    * (one.clone() - is_branch_child_prev.clone()) // to prevent Poisoned Constraint
                    * (one.clone() - not_first_level.clone())
                    * is_extension_node.clone()
                    * is_extension_c_row.clone();

            let s_rlp2 = meta.query_advice(s_rlp2, Rotation::prev());
            let s_advices0 = meta.query_advice(s_advices[0], Rotation::prev());
            let s_advices1 = meta.query_advice(s_advices[1], Rotation::prev());

            // skip 1 because s_advices[0] is 0 and doesn't contain any key info
            let mut first_level_long_even_rlc = s_advices1.clone() + compute_rlc(
                meta,
                s_advices.iter().skip(1).map(|v| *v).collect_vec(),
                0,
                one.clone(),
                -1,
                r_table.clone(),
            );
            first_level_long_even_rlc = first_level_long_even_rlc + modified_node_cur.clone() * c16.clone();
            constraints.push((
                "account first level long even",
                account_first.clone()
                    * is_key_even.clone()
                    * is_long.clone()
                    * (first_level_long_even_rlc.clone() - key_rlc_cur.clone())
            )); // TODO: prepare test

            let first_level_short_ext_rlc =
                (s_rlp2.clone() - c16.clone()) * c16.clone(); // -16 because of hexToCompact
            let first_level_short_branch_rlc = first_level_short_ext_rlc.clone() + modified_node_cur.clone();
            constraints.push((
                "account first level short extension",
                account_first.clone()
                * is_short.clone()
                    * (first_level_short_ext_rlc.clone() - key_rlc_cur.clone())
            )); // TODO: prepare test
            constraints.push((
                "account first level short branch",
                account_first.clone()
                    * is_short.clone()
                    * (first_level_short_branch_rlc.clone() - key_rlc_branch.clone())
            )); // TODO: prepare test
            constraints.push((
                "account first level short branch mult",
                account_first.clone()
                    * is_short.clone()
                    * (r_table[0].clone() - key_rlc_mult_branch.clone())
            )); // TODO: prepare test

            // TODO: long odd for first level account proof

            // First storage level:

            let storage_first = not_first_level.clone()
                    * is_account_leaf_storage_codehash_prev.clone()
                    * is_extension_node.clone()
                    * is_extension_c_row.clone();

            constraints.push((
                "storage first level long even",
                storage_first.clone()
                    * is_key_even.clone()
                    * is_long.clone()
                    * (first_level_long_even_rlc - key_rlc_cur.clone())
            )); // TODO: prepare test

            constraints.push((
                "storage first level short extension",
                storage_first.clone()
                    * is_short.clone()
                    * (first_level_short_ext_rlc - key_rlc_cur.clone())
            ));
            constraints.push((
                "storage first level short branch",
                storage_first.clone()
                    * is_short.clone()
                    * (first_level_short_branch_rlc - key_rlc_branch.clone())
            ));
            constraints.push((
                "storage first level short branch mult",
                storage_first.clone()
                    * is_short.clone()
                    * (r_table[0].clone() - key_rlc_mult_branch.clone())
            ));

            // Not first level:

            // long even not first level not first storage selector:
            let long_even = not_first_level.clone()
                    * (one.clone() - is_account_leaf_storage_codehash_prev.clone())
                    * is_extension_node.clone()
                    * is_extension_c_row.clone()
                    * is_key_even.clone()
                    * is_long.clone();

            let mut long_even_rlc_sel1 = key_rlc_prev_level.clone() +
                s_advices1 * key_rlc_mult_prev_level.clone();
            // skip 1 because s_advices[0] is 0 and doesn't contain any key info, and skip another 1
            // because s_advices[1] is not to be multiplied by any r_table element (as it's in compute_rlc).
            long_even_rlc_sel1 = long_even_rlc_sel1.clone() + compute_rlc(
                meta,
                s_advices.iter().skip(2).map(|v| *v).collect_vec(),
                0,
                key_rlc_mult_prev_level.clone(),
                -1,
                r_table.clone(),
            );
            constraints.push((
                "long even sel1 extension",
                    long_even.clone()
                    * sel1.clone()
                    * (key_rlc_cur.clone() - long_even_rlc_sel1.clone())
            ));
            // We check branch key RLC in extension C row too (otherwise +rotation would be needed
            // because we first have branch rows and then extension rows):
            constraints.push((
                "long even sel1 branch",
                    long_even.clone()
                    * sel1.clone()
                    * (key_rlc_branch.clone() - key_rlc_cur.clone() -
                        c16.clone() * modified_node_cur.clone() * key_rlc_mult_prev_level.clone() * mult_diff.clone())
            ));
            constraints.push((
                "long even sel1 branch mult",
                    long_even.clone()
                    * sel1.clone()
                    * (key_rlc_mult_branch.clone() - key_rlc_mult_prev_level.clone() * mult_diff.clone())
                    // mult_diff is checked in a lookup below
            ));

            /* 
            Note that there can be at max 31 key bytes because 32 same bytes would mean
            the two keys being the same - update operation, not splitting into extension node.
            So, we don't need to look further than s_advices even if the first byte (s_advices[0])
            is "padding".

            Example:
            bytes: [228, 130, 0, 9*16 + 5, 0, ...]
            nibbles: [5, 0, ...]
            Having sel2 means we need to:
                key_rlc_prev_level + first_nibble * key_rlc_mult_prev_level,
            However, we need to compute first_nibble (which is 9 in the example above).
            We get first_nibble by having second_nibble (5 in the example above) as the witness
            in extension row C and then: first_nibble = ((9*16 + 5) - 5) / 16.
            */
            let mut long_even_sel2_rlc = key_rlc_prev_level.clone();

            for ind in 0..HASH_WIDTH-1 {
                let s = meta.query_advice(s_advices[1+ind], Rotation::prev());
                let second_nibble = meta.query_advice(s_advices[ind], Rotation::cur());
                let first_nibble = (s.clone() - second_nibble.clone()) * c16inv.clone();
                // Note that first_nibble and second_nibble need to be between 0 and 15 - this
                // is checked in a lookup below.
                constraints.push((
                    "long even sel2 nibble correspond to byte",
                        long_even.clone()
                        * sel2.clone()
                        * (s - first_nibble.clone() * c16.clone() - second_nibble.clone())
                ));

                long_even_sel2_rlc = long_even_sel2_rlc +
                    first_nibble.clone() * key_rlc_mult_prev_level.clone();

                long_even_sel2_rlc = long_even_sel2_rlc +
                    second_nibble.clone() * c16.clone() * key_rlc_mult_prev_level.clone() * r_table[ind].clone();
            }
            constraints.push((
                "long even sel2 extension",
                    long_even.clone()
                    * sel2.clone()
                    * (key_rlc_cur.clone() - long_even_sel2_rlc.clone())
            ));
            // We check branch key RLC in extension C row too (otherwise +rotation would be needed
            // because we first have branch rows and then extension rows):
            constraints.push((
                "long even sel2 branch",
                    long_even.clone()
                    * sel2.clone()
                    * (key_rlc_branch.clone() - key_rlc_cur.clone() -
                        modified_node_cur.clone() * key_rlc_mult_prev_level.clone() * mult_diff.clone())
            ));
            constraints.push((
                "long even sel2 branch mult",
                    long_even
                    * sel2.clone()
                    * (key_rlc_mult_branch.clone() - key_rlc_mult_prev_level.clone() * mult_diff.clone() * r_table[0].clone())
                    // mult_diff is checked in a lookup below
            ));

            // long odd not first level not first storage selector:
            let long_odd = not_first_level.clone()
                    * (one.clone() - is_account_leaf_storage_codehash_prev.clone())
                    * is_extension_node.clone()
                    * is_extension_c_row.clone()
                    * is_key_odd.clone()
                    * is_long.clone();
            /* 
            Example:
            bytes: [228, 130, 16 + 3, 9*16 + 5, 0, ...]
            nibbles: [5, 0, ...]
            */
            let mut long_odd_sel1_rlc = key_rlc_prev_level.clone() +
                c16.clone() * (s_advices0.clone() - c16.clone()) * key_rlc_mult_prev_level.clone(); // -16 because of hexToCompact
            // s_advices0 - 16 = 3 in example above
            for ind in 0..HASH_WIDTH-1 {
                let s = meta.query_advice(s_advices[1+ind], Rotation::prev());
                let second_nibble = meta.query_advice(s_advices[ind], Rotation::cur());
                let first_nibble = (s.clone() - second_nibble.clone()) * c16inv.clone();
                // Note that first_nibble and second_nibble need to be between 0 and 15 - this
                // is checked in a lookup below.
                constraints.push((
                    "long odd sel1 nibble correspond to byte",
                        long_odd.clone()
                        * sel1.clone()
                        * (s - first_nibble.clone() * c16.clone() - second_nibble.clone())
                ));

                long_odd_sel1_rlc = long_odd_sel1_rlc +
                    first_nibble.clone() * key_rlc_mult_prev_level.clone();

                long_odd_sel1_rlc = long_odd_sel1_rlc +
                    second_nibble.clone() * c16.clone() * key_rlc_mult_prev_level.clone() * r_table[ind].clone();
            }
            constraints.push((
                "long odd sel1 extension",
                    long_odd.clone()
                    * sel1.clone()
                    * (key_rlc_cur.clone() - long_odd_sel1_rlc.clone())
            ));
            // We check branch key RLC in extension C row too (otherwise +rotation would be needed
            // because we first have branch rows and then extension rows):
            constraints.push((
                "long odd sel1 branch",
                    long_odd.clone()
                    * sel1.clone()
                    * (key_rlc_branch.clone() - key_rlc_cur.clone() -
                        modified_node_cur.clone() * key_rlc_mult_prev_level.clone() * mult_diff.clone())
            ));
            constraints.push((
                "long odd sel1 branch mult",
                    long_odd.clone()
                    * sel1.clone()
                    * (key_rlc_mult_branch.clone() - key_rlc_mult_prev_level.clone() * mult_diff.clone() * r_table[0].clone())
                    // mult_diff is checked in a lookup below
            ));
    
            /* 
            Example:
            bytes: [228, 130, 16 + 3, 137, 0, ...]
            nibbles: [5, 0, ...]
            */
            let mut long_odd_sel2_rlc = key_rlc_prev_level.clone() +
                (s_advices0 - c16.clone()) * key_rlc_mult_prev_level.clone();
            // skip 1 because s_advices[0] has already been taken into account
            long_odd_sel2_rlc = long_odd_sel2_rlc.clone() + compute_rlc(
                meta,
                s_advices.iter().skip(1).map(|v| *v).collect_vec(),
                0,
                key_rlc_mult_prev_level.clone(),
                -1,
                r_table.clone(),
            );
            constraints.push((
                "long odd sel2 extension",
                    long_odd.clone()
                    * sel2.clone()
                    * (key_rlc_cur.clone() - long_odd_sel2_rlc.clone())
            ));
            // We check branch key RLC in extension C row too (otherwise +rotation would be needed
            // because we first have branch rows and then extension rows):
            constraints.push((
                "long odd sel2 branch",
                    long_odd.clone()
                    * sel2.clone()
                    * (key_rlc_branch.clone() - key_rlc_cur.clone() -
                        c16.clone() * modified_node_cur.clone() * key_rlc_mult_prev_level.clone() * mult_diff.clone())
            ));
            constraints.push((
                "long odd sel2 branch mult",
                    long_odd.clone()
                    * sel2.clone()
                    * (key_rlc_mult_branch.clone() - key_rlc_mult_prev_level.clone() * mult_diff.clone())
                    // mult_diff is checked in a lookup below
            ));

            // short:

            let short = not_first_level.clone()
                    * (one.clone() - is_account_leaf_storage_codehash_prev.clone())
                    * is_extension_node.clone()
                    * is_extension_c_row.clone()
                    * is_short.clone();

            let short_sel1_rlc = key_rlc_prev_level.clone() +
                (s_rlp2.clone() - c16.clone()) * key_rlc_mult_prev_level.clone(); // -16 because of hexToCompact
            constraints.push((
                "short sel1 extension",
                    short.clone()
                    * sel1.clone()
                    * (key_rlc_cur.clone() - short_sel1_rlc.clone())
            ));
            // We check branch key RLC in extension C row too (otherwise +rotation would be needed
            // because we first have branch rows and then extension rows):
            constraints.push((
                "short sel1 branch",
                    short.clone()
                    * sel1.clone()
                    * (key_rlc_branch.clone() - key_rlc_cur.clone() -
                        c16.clone() * modified_node_cur.clone() * key_rlc_mult_prev_level.clone() * r_table[0].clone())
            ));
            constraints.push((
                "short sel1 branch mult",
                    short.clone()
                    * sel1.clone()
                    * (key_rlc_mult_branch.clone() - key_rlc_mult_prev_level.clone() * r_table[0].clone())
            ));

            let short_sel2_rlc = key_rlc_prev_level.clone() +
                c16.clone() * (s_rlp2 - c16.clone()) * key_rlc_mult_prev_level.clone(); // -16 because of hexToCompact
            constraints.push((
                "short sel2 extension",
                    short.clone()
                    * sel2.clone()
                    * (key_rlc_cur.clone() - short_sel2_rlc.clone())
            ));
            // We check branch key RLC in extension C row too (otherwise +rotation would be needed
            // because we first have branch rows and then extension rows):
            constraints.push((
                "short sel2 branch",
                    short.clone()
                    * sel2.clone()
                    * (key_rlc_branch.clone() - key_rlc_cur.clone() -
                        modified_node_cur.clone() * key_rlc_mult_prev_level.clone())
            ));
            constraints.push((
                "short sel2 branch mult",
                    short
                    * sel2.clone()
                    * (key_rlc_mult_branch.clone() - key_rlc_mult_prev_level.clone() * r_table[0].clone())
            ));

            constraints
        });

        let get_long_even = |meta: &mut VirtualCells<F>| {
            let is_extension_node = meta.query_advice(
                s_advices[IS_EXTENSION_NODE_POS - LAYOUT_OFFSET],
                Rotation(rot_into_branch_init),
            );
            // NOTE: is_key_even and is_key_odd is for number of nibbles that
            // are compactly encoded.
            let is_key_even = meta.query_advice(
                s_advices[IS_EXTENSION_EVEN_KEY_LEN_POS - LAYOUT_OFFSET],
                Rotation(rot_into_branch_init),
            );
            let is_long = meta.query_advice(
                s_advices[IS_EXTENSION_KEY_LONG_POS - LAYOUT_OFFSET],
                Rotation(rot_into_branch_init),
            );
            let is_account_leaf_storage_codehash_prev = meta.query_advice(
                is_account_leaf_storage_codehash_c,
                Rotation(rot_into_branch_init - 1),
            );
            let is_extension_c_row =
                meta.query_advice(is_last_branch_child, Rotation(-2));

            (one.clone() - is_account_leaf_storage_codehash_prev.clone())
                * is_extension_node.clone()
                * is_extension_c_row.clone()
                * is_key_even.clone()
                * is_long.clone()
        };

        let get_long_odd = |meta: &mut VirtualCells<F>| {
            let is_extension_node = meta.query_advice(
                s_advices[IS_EXTENSION_NODE_POS - LAYOUT_OFFSET],
                Rotation(rot_into_branch_init),
            );
            let is_key_odd = meta.query_advice(
                s_advices[IS_EXTENSION_ODD_KEY_LEN_POS - LAYOUT_OFFSET],
                Rotation(rot_into_branch_init),
            );
            let is_long = meta.query_advice(
                s_advices[IS_EXTENSION_KEY_LONG_POS - LAYOUT_OFFSET],
                Rotation(rot_into_branch_init),
            );
            let is_account_leaf_storage_codehash_prev = meta.query_advice(
                is_account_leaf_storage_codehash_c,
                Rotation(rot_into_branch_init - 1),
            );
            let is_extension_c_row =
                meta.query_advice(is_last_branch_child, Rotation(-2));

            (one.clone() - is_account_leaf_storage_codehash_prev.clone())
                * is_extension_node.clone()
                * is_extension_c_row.clone()
                * is_key_odd.clone()
                * is_long.clone()
        };

        // mult_diff
        meta.lookup_any(|meta| {
            let mut constraints = vec![];

            let sel1 = meta.query_advice(sel1, Rotation(rot_into_branch_init));
            let sel2 = meta.query_advice(sel2, Rotation(rot_into_branch_init));
            let long_even = get_long_even(meta);
            let long_odd = get_long_odd(meta);

            let long_even_sel1 = long_even.clone() * sel1.clone();
            let long_even_sel2 = long_even * sel2.clone();
            let long_odd_sel1 = long_odd.clone() * sel1;
            let long_odd_sel2 = long_odd * sel2;

            let s_rlp2 = meta.query_advice(s_rlp2, Rotation::prev());
            let key_len = s_rlp2 - c128.clone() - one.clone(); // -1 because long short has 0 in s_advices[0]
            let mult_diff = meta
                .query_advice(mult_diff, Rotation(rot_into_branch_init + 1));

            constraints.push((
                Expression::Constant(F::from(FixedTableTag::RMult as u64)),
                meta.query_fixed(fixed_table[0], Rotation::cur()),
            ));
            constraints.push((
                (long_even_sel1.clone()
                    + long_even_sel2.clone()
                    + long_odd_sel1.clone()
                    + long_odd_sel2.clone())
                    * key_len,
                meta.query_fixed(fixed_table[1], Rotation::cur()),
            ));
            constraints.push((
                (long_even_sel1
                    + long_even_sel2
                    + long_odd_sel1
                    + long_odd_sel2)
                    * mult_diff,
                meta.query_fixed(fixed_table[2], Rotation::cur()),
            ));

            constraints
        });

        // second_nibble needs to be between 0 and 15.
        for ind in 0..HASH_WIDTH - 1 {
            meta.lookup_any(|meta| {
                let mut constraints = vec![];

                let sel1 =
                    meta.query_advice(sel1, Rotation(rot_into_branch_init));
                let sel2 =
                    meta.query_advice(sel2, Rotation(rot_into_branch_init));
                let long_even_sel2 = get_long_even(meta) * sel2;
                let long_odd_sel1 = get_long_odd(meta) * sel1;
                let second_nibble =
                    meta.query_advice(s_advices[ind], Rotation::cur());

                constraints.push((
                    Expression::Constant(F::from(
                        FixedTableTag::Range16 as u64,
                    )),
                    meta.query_fixed(fixed_table[0], Rotation::cur()),
                ));
                constraints.push((
                    (long_even_sel2 + long_odd_sel1) * second_nibble,
                    meta.query_fixed(fixed_table[1], Rotation::cur()),
                ));

                constraints
            });
        }

        // first_nibble needs to be between 0 and 15.
        for ind in 0..HASH_WIDTH - 1 {
            meta.lookup_any(|meta| {
                let mut constraints = vec![];

                let sel1 =
                    meta.query_advice(sel1, Rotation(rot_into_branch_init));
                let sel2 =
                    meta.query_advice(sel2, Rotation(rot_into_branch_init));
                let long_even_sel2 = get_long_even(meta) * sel2;
                let long_odd_sel1 = get_long_odd(meta) * sel1;

                let s = meta.query_advice(s_advices[1 + ind], Rotation::prev());
                let second_nibble =
                    meta.query_advice(s_advices[ind], Rotation::cur());
                let first_nibble =
                    (s.clone() - second_nibble.clone()) * c16inv.clone();

                constraints.push((
                    Expression::Constant(F::from(
                        FixedTableTag::Range16 as u64,
                    )),
                    meta.query_fixed(fixed_table[0], Rotation::cur()),
                ));
                constraints.push((
                    (long_even_sel2 + long_odd_sel1) * first_nibble,
                    meta.query_fixed(fixed_table[1], Rotation::cur()),
                ));

                constraints
            });
        }

        config
    }

    pub fn construct(config: ExtensionNodeKeyConfig) -> Self {
        Self {
            config,
            _marker: PhantomData,
        }
    }
}

impl<F: FieldExt> Chip<F> for ExtensionNodeKeyChip<F> {
    type Config = ExtensionNodeKeyConfig;
    type Loaded = ();

    fn config(&self) -> &Self::Config {
        &self.config
    }

    fn loaded(&self) -> &Self::Loaded {
        &()
    }
}
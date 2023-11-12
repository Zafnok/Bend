use super::{var_id_to_name, Book, DefId, LetPat, Name, Op, Term, Val};
use crate::net::{INet, NodeId, NodeKind::*, Port, SlotId, ROOT};
use std::collections::{HashMap, HashSet};

// TODO: Display scopeless lambdas as such
/// Converts an Interaction-INet to a Lambda Calculus term, resolvind Dups and Sups where possible.
pub fn net_to_term_non_linear(net: &INet, book: &Book) -> (Term, bool) {
  /// Reads a term recursively by starting at root node.
  /// Returns the term and whether it's a valid readback.
  fn reader(
    net: &INet,
    next: Port,
    var_port_to_id: &mut HashMap<Port, Val>,
    id_counter: &mut Val,
    dup_scope: &mut HashMap<u8, Vec<SlotId>>,
    lets_vec: &mut Vec<NodeId>,
    lets_set: &mut HashSet<NodeId>,
    book: &Book,
  ) -> (Term, bool) {
    let node = next.node();

    match net.node(node).kind {
      // If we're visiting a set...
      Era => {
        // Only the main port actually exists in an ERA, the auxes are just an artifact of this representation.
        let valid = next.slot() == 0;
        (Term::Era, valid)
      }
      // If we're visiting a con node...
      Con => match next.slot() {
        // If we're visiting a port 0, then it is a lambda.
        0 => {
          let nam = decl_name(net, Port(node, 1), var_port_to_id, id_counter);
          let prt = net.enter_port(Port(node, 2));
          let (bod, valid) =
            reader(net, prt, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);
          (Term::Lam { nam, bod: Box::new(bod) }, valid)
        }
        // If we're visiting a port 1, then it is a variable.
        1 => (Term::Var { nam: var_name(next, var_port_to_id, id_counter) }, true),
        // If we're visiting a port 2, then it is an application.
        2 => {
          let prt = net.enter_port(Port(node, 0));
          let (fun, fun_valid) =
            reader(net, prt, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);
          let prt = net.enter_port(Port(node, 1));
          let (arg, arg_valid) =
            reader(net, prt, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);
          let valid = fun_valid && arg_valid;
          (Term::App { fun: Box::new(fun), arg: Box::new(arg) }, valid)
        }
        _ => unreachable!(),
      },
      Mat => match next.slot() {
        2 => {
          // Read the matched expression
          let cond_port = net.enter_port(Port(node, 0));
          let (cond_term, cond_valid) =
            reader(net, cond_port, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);

          // Read the pattern matching node
          let sel_node = net.enter_port(Port(node, 1)).node();

          // We expect the pattern matching node to be a CON
          let sel_kind = net.node(sel_node).kind;
          if sel_kind != Con {
            // TODO: Is there any case where we expect a different node type here on readback?
            return (
              Term::Match { cond: Box::new(cond_term), zero: Box::new(Term::Era), succ: Box::new(Term::Era) },
              false,
            );
          }

          let zero_port = net.enter_port(Port(sel_node, 1));
          let (zero_term, zero_valid) =
            reader(net, zero_port, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);
          let succ_port = net.enter_port(Port(sel_node, 2));
          let (succ_term, succ_valid) =
            reader(net, succ_port, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);

          let valid = cond_valid && zero_valid && succ_valid;
          (
            Term::Match { cond: Box::new(cond_term), zero: Box::new(zero_term), succ: Box::new(succ_term) },
            valid,
          )
        }
        _ => unreachable!(),
      },
      Ref { def_id } => {
        if book.is_generated_def(def_id) {
          let def = book.defs.get(&def_id).unwrap();
          def.assert_no_pattern_matching_rules();
          let mut term = def.rules[0].body.clone();
          term.fix_names(id_counter, book);

          (term, true)
        } else {
          (Term::Ref { def_id }, true)
        }
      }
      // If we're visiting a fan node...
      Dup { lab } => match next.slot() {
        // If we're visiting a port 0, then it is a pair.
        0 => {
          let stack = dup_scope.entry(lab).or_default();
          if let Some(slot) = stack.pop() {
            // Since we had a paired Dup in the path to this Sup,
            // we "decay" the superposition according to the original direction we came from the Dup.
            let chosen = net.enter_port(Port(node, slot));
            let (val, valid) =
              reader(net, chosen, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);
            dup_scope.get_mut(&lab).unwrap().push(slot);
            (val, valid)
          } else {
            // If no Dup with same label in the path, we can't resolve the Sup, so keep it as a term.
            let fst = net.enter_port(Port(node, 1));
            let snd = net.enter_port(Port(node, 2));
            let (fst, fst_valid) =
              reader(net, fst, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);
            let (snd, snd_valid) =
              reader(net, snd, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);
            let valid = fst_valid && snd_valid;
            (Term::Sup { fst: Box::new(fst), snd: Box::new(snd) }, valid)
          }
        }
        // If we're visiting a port 1 or 2, then it is a variable.
        // Also, that means we found a dup, so we store it to read later.
        1 | 2 => {
          let body = net.enter_port(Port(node, 0));
          dup_scope.entry(lab).or_default().push(next.slot());
          let (body, valid) =
            reader(net, body, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);
          dup_scope.entry(lab).or_default().pop().unwrap();
          (body, valid)
        }
        _ => unreachable!(),
      },
      Num { val } => (Term::Num { val }, true),
      Op2 => match next.slot() {
        2 => {
          let op_port = net.enter_port(Port(node, 0));
          let (op_term, op_valid) =
            reader(net, op_port, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);
          let arg_port = net.enter_port(Port(node, 1));
          let (arg_term, fst_valid) =
            reader(net, arg_port, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);
          let valid = op_valid && fst_valid;
          match op_term {
            Term::Num { val } => {
              let (val, op) = split_num_with_op(val);
              if let Some(op) = op {
                // This is Num + Op in the same value
                (Term::Opx { op, fst: Box::new(Term::Num { val }), snd: Box::new(arg_term) }, valid)
              } else {
                // This is just Op as value
                (
                  Term::Opx {
                    op: Op::from_hvmc_label(val).unwrap(),
                    fst: Box::new(arg_term),
                    snd: Box::new(Term::Era),
                  },
                  valid,
                )
              }
            }
            Term::Opx { op, fst, snd: _ } => (Term::Opx { op, fst, snd: Box::new(arg_term) }, valid),
            // TODO: Actually unreachable?
            _ => unreachable!(),
          }
        }
        _ => unreachable!(),
      },
      Rot => (Term::Era, false),
      Tup => match next.slot() {
        // If we're visiting a port 0, then it is a Tup.
        0 => {
          let fst = net.enter_port(Port(node, 1));
          let snd = net.enter_port(Port(node, 2));
          let (fst, fst_valid) =
            reader(net, fst, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);
          let (snd, snd_valid) =
            reader(net, snd, var_port_to_id, id_counter, dup_scope, lets_vec, lets_set, book);
          let valid = fst_valid && snd_valid;
          (Term::Tup { fst: Box::new(fst), snd: Box::new(snd) }, valid)
        }
        // If we're visiting a port 1 or 2, then it is a variable.
        // Also, that means we found a let, so we store it to read later.
        1 | 2 => {
          if !lets_set.contains(&node) {
            lets_set.insert(node);
            lets_vec.push(node);
          } else {
            // Second time we find, it has to be the other let variable.
          }
          (Term::Var { nam: var_name(next, var_port_to_id, id_counter) }, true)
        }
        _ => unreachable!(),
      },
    }
  }
  // A hashmap linking ports to binder names. Those ports have names:
  // Port 1 of a con node (λ), ports 1 and 2 of a fan node (let).
  let mut var_port_to_id = HashMap::new();
  let id_counter = &mut 0;
  let mut dup_scope = HashMap::new();
  let mut lets_vec = Vec::new();
  let mut lets_set = HashSet::new();

  // Reads the main term from the net
  let (mut main, mut valid) = reader(
    net,
    net.enter_port(ROOT),
    &mut var_port_to_id,
    id_counter,
    &mut dup_scope,
    &mut lets_vec,
    &mut lets_set,
    book,
  );

  while let Some(tup) = lets_vec.pop() {
    let val = net.enter_port(Port(tup, 0));
    let (val, val_valid) =
      reader(net, val, &mut var_port_to_id, id_counter, &mut dup_scope, &mut lets_vec, &mut lets_set, book);
    let fst = decl_name(net, Port(tup, 1), &mut var_port_to_id, id_counter);
    let snd = decl_name(net, Port(tup, 2), &mut var_port_to_id, id_counter);
    main = Term::Let { pat: LetPat::Tup(fst, snd), val: Box::new(val), nxt: Box::new(main) };
    valid = valid && val_valid;
  }

  (main, valid)
}

/// Converts an Interaction-INet to an Interaction Calculus term.
pub fn net_to_term_linear(net: &INet, book: &Book) -> (Term, bool) {
  /// Reads a term recursively by starting at root node.
  /// Returns the term and whether it's a valid readback.
  fn reader(
    net: &INet,
    next: Port,
    var_port_to_id: &mut HashMap<Port, Val>,
    id_counter: &mut Val,
    dups_vec: &mut Vec<NodeId>,
    dups_set: &mut HashSet<NodeId>,
    seen: &mut HashSet<Port>,
    book: &Book,
  ) -> (Term, bool) {
    if seen.contains(&next) {
      return (Term::Var { nam: Name::new("...") }, false);
    }
    seen.insert(next);

    let node = next.node();

    match net.node(node).kind {
      // If we're visiting a set...
      Era => {
        // Only the main port actually exists in an ERA, the auxes are just an artifact of this representation.
        let valid = next.slot() == 0;
        (Term::Era, valid)
      }
      // If we're visiting a con node...
      Con => match next.slot() {
        // If we're visiting a port 0, then it is a lambda.
        0 => {
          seen.insert(Port(node, 2));
          let nam = decl_name(net, Port(node, 1), var_port_to_id, id_counter);
          let prt = net.enter_port(Port(node, 2));
          let (bod, valid) = reader(net, prt, var_port_to_id, id_counter, dups_vec, dups_set, seen, book);
          (Term::Lam { nam, bod: Box::new(bod) }, valid)
        }
        // If we're visiting a port 1, then it is a variable.
        1 => (Term::Var { nam: var_name(next, var_port_to_id, id_counter) }, true),
        // If we're visiting a port 2, then it is an application.
        2 => {
          seen.insert(Port(node, 0));
          seen.insert(Port(node, 1));
          let prt = net.enter_port(Port(node, 0));
          let (fun, fun_valid) = reader(net, prt, var_port_to_id, id_counter, dups_vec, dups_set, seen, book);
          let prt = net.enter_port(Port(node, 1));
          let (arg, arg_valid) = reader(net, prt, var_port_to_id, id_counter, dups_vec, dups_set, seen, book);
          let valid = fun_valid && arg_valid;
          (Term::App { fun: Box::new(fun), arg: Box::new(arg) }, valid)
        }
        _ => unreachable!(),
      },
      Mat => match next.slot() {
        2 => {
          // Read the matched expression
          seen.insert(Port(node, 0));
          seen.insert(Port(node, 1));
          let cond_port = net.enter_port(Port(node, 0));
          let (cond_term, cond_valid) =
            reader(net, cond_port, var_port_to_id, id_counter, dups_vec, dups_set, seen, book);

          // Read the pattern matching node
          let sel_node = net.enter_port(Port(node, 1)).node();
          seen.insert(Port(sel_node, 0));
          seen.insert(Port(sel_node, 1));
          seen.insert(Port(sel_node, 2));

          // We expect the pattern matching node to be a CON
          let sel_kind = net.node(sel_node).kind;
          if sel_kind != Con {
            // TODO: Is there any case where we expect a different node type here on readback?
            return (
              Term::Match { cond: Box::new(cond_term), zero: Box::new(Term::Era), succ: Box::new(Term::Era) },
              false,
            );
          }

          let zero_port = net.enter_port(Port(sel_node, 1));
          let (zero_term, zero_valid) =
            reader(net, zero_port, var_port_to_id, id_counter, dups_vec, dups_set, seen, book);
          let succ_port = net.enter_port(Port(sel_node, 2));
          let (succ_term, succ_valid) =
            reader(net, succ_port, var_port_to_id, id_counter, dups_vec, dups_set, seen, book);

          let valid = cond_valid && zero_valid && succ_valid;
          (
            Term::Match { cond: Box::new(cond_term), zero: Box::new(zero_term), succ: Box::new(succ_term) },
            valid,
          )
        }
        _ => unreachable!(),
      },
      Ref { def_id } => {
        if book.is_generated_def(def_id) {
          let def = book.defs.get(&def_id).unwrap();
          def.assert_no_pattern_matching_rules();
          let mut term = def.rules[0].body.clone();
          term.fix_names(id_counter, book);

          (term, true)
        } else {
          (Term::Ref { def_id }, true)
        }
      }
      // If we're visiting a fan node...
      Dup { lab: _ } => match next.slot() {
        // If we're visiting a port 0, then it is a pair.
        0 => {
          seen.insert(Port(node, 1));
          seen.insert(Port(node, 2));
          let fst_port = net.enter_port(Port(node, 1));
          let (fst, fst_valid) =
            reader(net, fst_port, var_port_to_id, id_counter, dups_vec, dups_set, seen, book);
          let snd_port = net.enter_port(Port(node, 2));
          let (snd, snd_valid) =
            reader(net, snd_port, var_port_to_id, id_counter, dups_vec, dups_set, seen, book);
          let valid = fst_valid && snd_valid;
          (Term::Sup { fst: Box::new(fst), snd: Box::new(snd) }, valid)
        }
        // If we're visiting a port 1 or 2, then it is a variable.
        // Also, that means we found a dup, so we store it to read later.
        1 | 2 => {
          if !dups_set.contains(&node) {
            dups_set.insert(node);
            dups_vec.push(node);
          } else {
            // Second time we find, it has to be the other dup variable.
          }
          (Term::Var { nam: var_name(next, var_port_to_id, id_counter) }, true)
        }
        _ => unreachable!(),
      },
      Num { val } => (Term::Num { val }, true),
      Op2 => match next.slot() {
        2 => {
          seen.insert(Port(node, 0));
          seen.insert(Port(node, 1));
          let op_port = net.enter_port(Port(node, 0));
          let (op_term, op_valid) =
            reader(net, op_port, var_port_to_id, id_counter, dups_vec, dups_set, seen, book);
          let arg_port = net.enter_port(Port(node, 1));
          let (arg_term, fst_valid) =
            reader(net, arg_port, var_port_to_id, id_counter, dups_vec, dups_set, seen, book);
          let valid = op_valid && fst_valid;
          match op_term {
            Term::Num { val } => {
              let (val, op) = split_num_with_op(val);
              if let Some(op) = op {
                // This is Num + Op in the same value
                (Term::Opx { op, fst: Box::new(Term::Num { val }), snd: Box::new(arg_term) }, valid)
              } else {
                // This is just Op as value
                (
                  Term::Opx {
                    op: Op::from_hvmc_label(val).unwrap(),
                    fst: Box::new(arg_term),
                    snd: Box::new(Term::Era),
                  },
                  valid,
                )
              }
            }
            Term::Opx { op, fst, snd: _ } => (Term::Opx { op, fst, snd: Box::new(arg_term) }, valid),
            // TODO: Actually unreachable?
            _ => unreachable!(),
          }
        }
        _ => unreachable!(),
      },
      // If we're revisiting the root node something went wrong with this net
      Rot => (Term::Era, false),
      Tup => {
        seen.insert(Port(node, 1));
        seen.insert(Port(node, 2));
        let fst_port = net.enter_port(Port(node, 1));
        let (fst, fst_valid) =
          reader(net, fst_port, var_port_to_id, id_counter, dups_vec, dups_set, seen, book);
        let snd_port = net.enter_port(Port(node, 2));
        let (snd, snd_valid) =
          reader(net, snd_port, var_port_to_id, id_counter, dups_vec, dups_set, seen, book);
        let valid = fst_valid && snd_valid;
        (Term::Tup { fst: Box::new(fst), snd: Box::new(snd) }, valid)
      }
    }
  }

  // A hashmap linking ports to binder names. Those ports have names:
  // Port 1 of a con node (λ), ports 1 and 2 of a fan node (let).
  let mut var_port_to_id = HashMap::new();
  let id_counter = &mut 0;

  // Dup aren't scoped. We find them when we read one of the variables
  // introduced by them. Thus, we must store the dups we find to read later.
  // We have a vec for .pop(). and a set to avoid storing duplicates.
  let mut dups_vec = Vec::new();
  let mut dups_set = HashSet::new();
  let mut seen = HashSet::new();

  // Reads the main term from the net
  let (mut main, mut valid) = reader(
    net,
    net.enter_port(ROOT),
    &mut var_port_to_id,
    id_counter,
    &mut dups_vec,
    &mut dups_set,
    &mut seen,
    book,
  );

  // Read all the dup bodies.
  while let Some(dup) = dups_vec.pop() {
    seen.insert(Port(dup, 0));
    let val = net.enter_port(Port(dup, 0));
    let (val, val_valid) =
      reader(net, val, &mut var_port_to_id, id_counter, &mut dups_vec, &mut dups_set, &mut seen, book);
    let fst = decl_name(net, Port(dup, 1), &mut var_port_to_id, id_counter);
    let snd = decl_name(net, Port(dup, 2), &mut var_port_to_id, id_counter);
    main = Term::Dup { fst, snd, val: Box::new(val), nxt: Box::new(main) };
    valid = valid && val_valid;
  }

  // Check if the readback didn't leave any unread nodes (for example reading var from a lam but never reading the lam itself)
  for &decl_port in var_port_to_id.keys() {
    for check_slot in 0 .. 3 {
      let check_port = Port(decl_port.node(), check_slot);
      let other_node = net.enter_port(check_port).node();
      if !seen.contains(&check_port) && net.node(other_node).kind != Era {
        valid = false;
      }
    }
  }

  (main, valid)
}

impl Op {
  pub fn from_hvmc_label(value: Val) -> Option<Op> {
    match value {
      0x1 => Some(Op::ADD),
      0x2 => Some(Op::SUB),
      0x3 => Some(Op::MUL),
      0x4 => Some(Op::DIV),
      0x5 => Some(Op::MOD),
      0x6 => Some(Op::EQ),
      0x7 => Some(Op::NE),
      0x8 => Some(Op::LT),
      0x9 => Some(Op::GT),
      0xa => Some(Op::AND),
      0xb => Some(Op::OR),
      0xc => Some(Op::XOR),
      0xd => Some(Op::NOT),
      0xe => Some(Op::LSH),
      0xf => Some(Op::RSH),
      _ => None,
    }
  }
}

// Given a port, returns its name, or assigns one if it wasn't named yet.
fn var_name(var_port: Port, var_port_to_id: &mut HashMap<Port, Val>, id_counter: &mut Val) -> Name {
  let id = var_port_to_id.entry(var_port).or_insert_with(|| {
    let id = *id_counter;
    *id_counter += 1;
    id
  });

  var_id_to_name(*id)
}

fn decl_name(
  net: &INet,
  var_port: Port,
  var_port_to_id: &mut HashMap<Port, Val>,
  id_counter: &mut Val,
) -> Option<Name> {
  // If port is linked to an erase node, return an unused variable
  let var_use = net.enter_port(var_port);
  let var_kind = net.node(var_use.node()).kind;
  if let Era = var_kind { None } else { Some(var_name(var_port, var_port_to_id, id_counter)) }
}

fn split_num_with_op(num: Val) -> (Val, Option<Op>) {
  let op = Op::from_hvmc_label(num >> 24);
  let num = num & ((1 << 24) - 1);
  (num, op)
}

impl Book {
  pub fn is_generated_def(&self, def_id: DefId) -> bool {
    self.def_names.name(&def_id).map_or(false, |Name(name)| name.contains('$'))
  }
}

impl Term {
  fn fix_names(&mut self, id_counter: &mut Val, book: &Book) {
    fn fix_name(nam: &mut Option<Name>, id_counter: &mut Val, bod: &mut Term) {
      if let Some(nam) = nam {
        let name = var_id_to_name(*id_counter);
        *id_counter += 1;
        bod.subst(nam, &Term::Var { nam: name.clone() });
        *nam = name;
      }
    }

    match self {
      Term::Lam { nam, bod } => {
        fix_name(nam, id_counter, bod);
        bod.fix_names(id_counter, book);
      }
      Term::Ref { def_id } => {
        if book.is_generated_def(*def_id) {
          let def = book.defs.get(def_id).unwrap();
          def.assert_no_pattern_matching_rules();
          let mut term = def.rules[0].body.clone();
          term.fix_names(id_counter, book);
          *self = term
        }
      }
      Term::Dup { fst, snd, val, nxt } => {
        val.fix_names(id_counter, book);
        fix_name(fst, id_counter, nxt);
        fix_name(snd, id_counter, nxt);
        nxt.fix_names(id_counter, book);
      }
      Term::Chn { nam: _, bod } => bod.fix_names(id_counter, book),
      Term::App { fun, arg } => {
        fun.fix_names(id_counter, book);
        arg.fix_names(id_counter, book);
      }
      Term::Sup { fst, snd } | Term::Tup { fst, snd } | Term::Opx { op: _, fst, snd } => {
        fst.fix_names(id_counter, book);
        snd.fix_names(id_counter, book);
      }
      Term::Match { cond, zero, succ } => {
        cond.fix_names(id_counter, book);
        zero.fix_names(id_counter, book);
        succ.fix_names(id_counter, book);
      }
      Term::Let { .. } => unreachable!(),
      Term::Var { .. } | Term::Lnk { .. } | Term::Num { .. } | Term::Era => {}
    }
  }
}
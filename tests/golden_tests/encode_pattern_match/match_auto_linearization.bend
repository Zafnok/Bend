# Given a match/switch term preceded by a sequence of terms with binds (lambda, let, use, etc),
# by default we linearize all bindings up to the first one that appears in the match "header".

switch_linearization =
  @a
  @b
  let c = 2;
  let {c1 c2} = c;
  use d = (a, b);
  let (e, f) = d;
  switch a {
    0: (b c c1 c2 d e f)
    _: (a-1 b c c1 c2 d e f)
  }

match_linearization =
  @a
  @b
  let c = 2;
  let {c1 c2} = c;
  use d = (a, b);
  let (e, f) = d;
  match a {
    Nat/Zero: (b c c1 c2 d e f)
    Nat/Succ: (a.pred b c c1 c2 d e f)
  }

switch_shadowed_field = @a @a-1 switch a {
  0: a-1
  _: a-1
}

match_shadowed_field = @a @a.head @a.tail match a {
  List/Nil: (List/Cons a.head a.tail)
  List/Cons: (List/Cons a.head a.tail)
}

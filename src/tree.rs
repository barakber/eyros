use desert::{ToBytes,FromBytes};
use crate::{Scalar,Point,Value,Coord,Location,query::QStream,Error,Storage};
use async_std::{sync::{Arc,Mutex}};
use crate::unfold::unfold;
use random_access_storage::RandomAccess;

pub type TreeRef = u64;

macro_rules! impl_branch {
  ($Tree:ident,$Branch:ident,$Node:ident,($($T:tt),+),($($i:tt),+),
  ($($j:tt),+),($($k:tt),+),($($cf:tt),+),
  ($($u:ty),+),($($n:tt),+),$dim:expr) => {
    #[derive(Debug)]
    pub enum $Node<$($T),+,V> where $($T: Scalar),+, V: Value {
      Branch($Branch<$($T),+,V>),
      Data(Vec<(($(Coord<$T>),+),V)>),
      Ref(TreeRef)
    }
    #[derive(Debug)]
    pub struct $Branch<$($T),+,V> where $($T: Scalar),+, V: Value {
      pub pivots: ($(Option<Vec<$T>>),+),
      pub intersections: Vec<Arc<$Node<$($T),+,V>>>,
      pub nodes: Vec<Arc<$Node<$($T),+,V>>>,
    }

    impl<$($T),+,V> $Branch<$($T),+,V> where $($T: Scalar),+, V: Value {
      fn dim() -> usize { 2 }
      pub fn build(branch_factor: usize, inserts: Arc<Vec<(&($(Coord<$T>),+),&V)>>)
      -> $Node<$($T),+,V> {
        let sorted = ($(
          {
            let mut xs: Vec<usize> = (0..inserts.len()).collect();
            xs.sort_unstable_by(|a,b| {
              coord_cmp(&(inserts[*a].0).$i,&(inserts[*b].0).$i).unwrap()
            });
            xs
          }
        ),+);
        let mut max_depth = 0;
        let root = Self::from_sorted(
          branch_factor, 0, Arc::clone(&inserts),
          ($(sorted.$i),+),
          &mut vec![false;inserts.len()],
          &mut max_depth
        );
        //eprintln!["max_depth={}", max_depth];
        root
      }
      fn from_sorted(branch_factor: usize, level: usize,
      inserts: Arc<Vec<(&($(Coord<$T>),+),&V)>>, sorted: ($(Vec<$u>),+),
      matched: &mut [bool], max_depth: &mut usize) -> $Node<$($T),+,V> {
        *max_depth = level.max(*max_depth);
        if sorted.0.len() == 0 {
          return $Node::Data(vec![]);
        } else if sorted.0.len() < branch_factor {
          return $Node::Data(sorted.0.iter().map(|i| {
            matched[*i] = true;
            let pv = &inserts[*i];
            (($((pv.0).$i.clone()),+),pv.1.clone())
          }).collect());
        }
        let n = (branch_factor-1).min(sorted.0.len()-1); // number of pivots
        let is_min = (level / Self::dim()) % 2 != 0;
        let mut pivots = ($($n),+);
        match level % Self::dim() {
          $($i => {
            let mut ps = match sorted.$i.len() {
              0 => panic!["not enough data to create a branch"],
              1 => match &(inserts[sorted.$i[0]].0).$i {
                Coord::Scalar(x) => {
                  vec![find_separation(x,x,x,x,is_min)]
                },
                Coord::Interval(min,max) => {
                  vec![find_separation(min,max,min,max,is_min)]
                }
              },
              2 => {
                let a = match &(inserts[sorted.$i[0]].0).$i {
                  Coord::Scalar(x) => (x,x),
                  Coord::Interval(min,max) => (min,max),
                };
                let b = match &(inserts[sorted.$i[1]].0).$i {
                  Coord::Scalar(x) => (x,x),
                  Coord::Interval(min,max) => (min,max),
                };
                vec![find_separation(a.0,a.1,b.0,b.1,is_min)]
              },
              _ => {
                (0..n).map(|k| {
                  let m = k * sorted.$i.len() / (n+1);
                  let a = match &(inserts[sorted.$i[m+0]].0).$i {
                    Coord::Scalar(x) => (x,x),
                    Coord::Interval(min,max) => (min,max),
                  };
                  let b = match &(inserts[sorted.$i[m+1]].0).$i {
                    Coord::Scalar(x) => (x,x),
                    Coord::Interval(min,max) => (min,max),
                  };
                  find_separation(a.0,a.1,b.0,b.1,is_min)
                }).collect()
              }
            };
            ps.sort_unstable_by(|a,b| {
              a.partial_cmp(b).unwrap()
            });
            pivots.$i = Some(ps);
          }),+,
          _ => panic!["unexpected level modulo dimension"]
        };

        let intersections: Vec<Arc<$Node<$($T),+,V>>> = match level % Self::dim() {
          $($i => pivots.$i.as_ref().unwrap().iter().map(|pivot| {
            let new_sorted = Self::filter_sorted(
              pivot,
              &sorted,
              matched,
              Arc::clone(&inserts),
              Box::new(|pivot, inserts, j: &usize| {
                intersect_pivot(&(inserts[*j].0).$i, pivot)
              })
            );
            if new_sorted.0.len() == sorted.0.len() {
              return Arc::new($Node::Data(new_sorted.0.iter().map(|i| {
                let pv = &inserts[*i];
                matched[*i] = true;
                (pv.0.clone(),pv.1.clone())
              }).collect()));
            }
            let b = $Branch::from_sorted(
              branch_factor,
              level+1,
              Arc::clone(&inserts),
              new_sorted,
              matched,
              max_depth
            );
            Arc::new(b)
          }).collect()),+,
          _ => panic!["unexpected level modulo dimension"]
        };

        let nodes = match level % Self::dim() {
          $($i => {
            let pv = pivots.$i.as_ref().unwrap();
            let mut nodes = Vec::with_capacity(pv.len()+1);
            nodes.push({
              let pivot = pv.first().unwrap();
              let next_sorted = Self::filter_sorted(
                pivot,
                &sorted,
                matched,
                Arc::clone(&inserts),
                Box::new(|pivot, inserts, j: &usize| {
                  coord_cmp_pivot(&(inserts[*j].0).$i, pivot)
                    == Some(std::cmp::Ordering::Less)
                })
              );
              Arc::new($Branch::from_sorted(
                branch_factor,
                level+1,
                Arc::clone(&inserts),
                next_sorted,
                matched,
                max_depth
              ))
            });
            let ranges = pv.iter().zip(pv.iter().skip(1));
            for (start,end) in ranges {
              let next_sorted = Self::filter_sorted_range(
                (start,end),
                &sorted,
                matched,
                Arc::clone(&inserts),
                Box::new(|(start,end), inserts, j: &usize| {
                  intersect_coord(&(inserts[*j].0).$i, start, end)
                })
              );
              nodes.push(Arc::new($Branch::from_sorted(
                branch_factor,
                level+1,
                Arc::clone(&inserts),
                next_sorted,
                matched,
                max_depth
              )));
            }
            if pv.len() > 1 {
              nodes.push({
                let pivot = pv.first().unwrap();
                let next_sorted = Self::filter_sorted(
                  pivot,
                  &sorted,
                  matched,
                  Arc::clone(&inserts),
                  Box::new(|pivot, inserts, j: &usize| {
                    coord_cmp_pivot(&(inserts[*j].0).$i, pivot)
                      == Some(std::cmp::Ordering::Greater)
                  })
                );
                Arc::new($Branch::from_sorted(
                  branch_factor,
                  level+1,
                  Arc::clone(&inserts),
                  next_sorted,
                  matched,
                  max_depth
                ))
              });
            }
            nodes
          }),+,
          _ => panic!["unexpected level modulo dimension"]
        };

        let node_count = nodes.iter().fold(0usize, |count,node| {
          count + match node.as_ref() {
            $Node::Data(bs) => if bs.is_empty() { 0 } else { 1 },
            $Node::Branch(_) => 1,
            $Node::Ref(_) => 1,
          }
        });
        if node_count <= 1 {
          return $Node::Data(sorted.0.iter().map(|i| {
            let (p,v) = &inserts[*i];
            matched[*i] = true;
            ((*p).clone(),(*v).clone())
          }).collect());
        }

        $Node::Branch(Self {
          pivots,
          intersections,
          nodes,
        })
      }
      fn filter_sorted<X>(pivot: &X, sorted: &($(Vec<$u>),+),
      matched: &[bool], inserts: Arc<Vec<(&($(Coord<$T>),+),&V)>>,
      f: Box<dyn Fn (&X,Arc<Vec<(&($(Coord<$T>),+),&V)>>, &usize) -> bool>)
      -> ($(Vec<$u>),+) where X: Scalar {
        ($({
          sorted.$i.iter()
            .map(|j| *j)
            .filter(|j| !matched[*j])
            .fold((&inserts,vec![]), |(inserts, mut res), j| {
              if f(pivot, Arc::clone(inserts), &j) { res.push(j) }
              (inserts,res)
            }).1
        }),+)
      }
      fn filter_sorted_range<X>(range: (&X,&X), sorted: &($(Vec<$u>),+),
      matched: &[bool], inserts: Arc<Vec<(&($(Coord<$T>),+),&V)>>,
      f: Box<dyn Fn ((&X,&X),Arc<Vec<(&($(Coord<$T>),+),&V)>>, &usize) -> bool>)
      -> ($(Vec<$u>),+) where X: Scalar, $($T: Scalar),+ {
        ($({
          sorted.$i.iter()
            .map(|j| *j)
            .filter(|j| !matched[*j])
            .fold((&inserts,vec![]), |(inserts, mut res), j| {
              if f(range, Arc::clone(inserts), &j) { res.push(j) }
              (inserts,res)
            }).1
        }),+)
      }
    }

    #[derive(Debug)]
    pub struct $Tree<$($T),+,V> where $($T: Scalar),+, V: Value {
      pub root: Arc<$Node<$($T),+,V>>,
      pub bounds: ($($T),+,$($T),+),
      pub count: usize,
    }

    #[async_trait::async_trait]
    impl<$($T),+,V> Tree<($(Coord<$T>),+),V> for $Tree<$($T),+,V> where $($T: Scalar),+, V: Value {
      fn build(branch_factor: usize, rows: Arc<Vec<(&($(Coord<$T>),+),&V)>>) -> Self {
        let ibounds = ($(
          match (rows[0].0).$k.clone() {
            Coord::Scalar(x) => x,
            Coord::Interval(x,_) => x,
          }
        ),+);
        Self {
          root: Arc::new($Branch::build(branch_factor, Arc::clone(&rows))),
          count: rows.len(),
          bounds: rows[1..].iter().fold(ibounds, |bounds,row| {
            ($($cf(&(row.0).$k, &bounds.$j)),+)
          })
        }
      }
      fn list(&mut self) -> (Vec<(($(Coord<$T>),+),V)>,Vec<TreeRef>) {
        let mut cursors = vec![Arc::clone(&self.root)];
        let mut rows = vec![];
        let mut refs = vec![];
        while let Some(c) = cursors.pop() {
          match c.as_ref() {
            $Node::Branch(branch) => {
              for b in branch.intersections.iter() {
                cursors.push(Arc::clone(b));
              }
              for b in branch.nodes.iter() {
                cursors.push(Arc::clone(b));
              }
            },
            $Node::Data(data) => {
              rows.extend(data.iter().map(|pv| {
                (pv.0.clone(),pv.1.clone())
              }).collect::<Vec<_>>());
            },
            $Node::Ref(r) => {
              refs.push(*r);
            },
          }
        }
        (rows,refs)
      }
      fn query<S>(&mut self, storage: Arc<Mutex<Box<dyn Storage<S>+Unpin+Send+Sync>>>,
      bbox: &(($($T),+),($($T),+))) -> Arc<Mutex<QStream<($(Coord<$T>),+),V>>>
      where S: RandomAccess<Error=Error>+Unpin+Send+Sync+'static {
        let istate = (
          bbox.clone(),
          vec![], // queue
          vec![(0usize,Arc::clone(&self.root))], // cursors
          vec![], // refs
          Arc::clone(&storage), // storage
        );
        Arc::new(Mutex::new(Box::new(unfold(istate, async move |mut state| {
          let bbox = &state.0;
          let queue = &mut state.1;
          let cursors = &mut state.2;
          let refs = &mut state.3;
          let storage = &mut state.4;
          loop {
            if let Some(q) = queue.pop() {
              return Some((Ok(q),state));
            }
            if cursors.is_empty() && !refs.is_empty() {
              // TODO: use a tree LRU
              match Self::load(Arc::clone(storage), refs.pop().unwrap()).await {
                Err(e) => return Some((Err(e.into()),state)),
                Ok(tree) => cursors.push((0usize,tree.root)),
              };
              continue;
            } else if cursors.is_empty() {
              return None;
            }
            let (level,c) = cursors.pop().unwrap();
            match c.as_ref() {
              $Node::Branch(branch) => {
                match level % $dim {
                  $($i => {
                    let pivots = branch.pivots.$i.as_ref().unwrap();
                    for (pivot,b) in pivots.iter().zip(branch.intersections.iter()) {
                      if &(bbox.0).$i <= pivot && pivot <= &(bbox.1).$i {
                        cursors.push((level+1,Arc::clone(b)));
                      }
                    }
                    let xs = &branch.nodes;
                    let ranges = pivots.iter().zip(pivots.iter().skip(1));
                    if &(bbox.0).$i <= pivots.first().unwrap() {
                      cursors.push((level+1,Arc::clone(xs.first().unwrap())));
                    }
                    for ((start,end),b) in ranges.zip(xs.iter().skip(1)) {
                      if intersect_iv(start, end, &(bbox.0).$i, &(bbox.1).$i) {
                        cursors.push((level+1,Arc::clone(b)));
                      }
                    }
                    if &(bbox.1).$i >= pivots.last().unwrap() {
                      cursors.push((level+1,Arc::clone(xs.last().unwrap())));
                    }
                  }),+
                  _ => panic!["unexpected level modulo dimension"]
                }
              },
              $Node::Data(data) => {
                queue.extend(data.iter()
                  .filter(|pv| {
                    intersect_coord(&(pv.0).0, &(bbox.0).0, &(bbox.1).0)
                    && intersect_coord(&(pv.0).1, &(bbox.0).1, &(bbox.1).1)
                  })
                  .map(|pv| {
                    let loc: Location = (0,0); // TODO
                    (pv.0.clone(),pv.1.clone(),loc)
                  })
                  .collect::<Vec<_>>()
                );
              },
              $Node::Ref(r) => {
                refs.push(*r);
              }
            }
          }
        }))))
      }
    }

    impl<$($T),+,V> $Tree<$($T),+,V> where $($T: Scalar),+, V: Value {
      async fn load<S>(storage: Arc<Mutex<Box<dyn Storage<S>+Unpin+Send+Sync>>>,
      r: TreeRef) -> Result<Self,Error> where S: RandomAccess<Error=Error>+Unpin+Send+Sync {
        let mut s = storage.lock().await.open(&format!["tree/{}",r.to_string()]).await?;
        let bytes = s.read(0, s.len().await?).await?;
        Ok(Self::from_bytes(&bytes)?.1)
      }
    }
  }
}

impl_branch![Tree2,Branch2,Node2,(P0,P1),(0,1),
  (0,1,2,3),(0,1,0,1),(coord_min,coord_min,coord_max,coord_max),
  (usize,usize),(None,None),2
];
impl_branch![Tree3,Branch3,Node3,(P0,P1,P2),(0,1,2),
  (0,1,2,3,4,5),(0,1,2,0,1,2),(coord_min,coord_min,coord_min,coord_max,coord_max,coord_max),
  (usize,usize,usize),(None,None,None),3
];

#[async_trait::async_trait]
pub trait Tree<P,V>: Send+Sync+ToBytes where P: Point, V: Value {
  fn build(branch_factor: usize, rows: Arc<Vec<(&P,&V)>>) -> Self where Self: Sized;
  fn list(&mut self) -> (Vec<(P,V)>,Vec<TreeRef>);
  fn query<S>(&mut self, storage: Arc<Mutex<Box<dyn Storage<S>+Unpin+Send+Sync>>>,
    bbox: &P::Bounds) -> Arc<Mutex<QStream<P,V>>>
    where S: RandomAccess<Error=Error>+Unpin+Send+Sync+'static;
}

pub async fn merge<T,P,V>(branch_factor: usize, inserts: &[(&P,&V)], trees: &[Arc<Mutex<T>>]) -> T
where P: Point, V: Value, T: Tree<P,V> {
  let mut lists = vec![];
  let mut refs = vec![];
  for tree in trees.iter() {
    let (list,xrefs) = tree.lock().await.list();
    lists.push(list);
    refs.extend(xrefs);
  }
  let mut rows = vec![];
  rows.extend_from_slice(inserts);
  for list in lists.iter_mut() {
    rows.extend(list.iter().map(|pv| {
      (&pv.0,&pv.1)
    }).collect::<Vec<_>>());
  }
  // TODO: merge overlapping refs
  // TODO: split large intersecting buckets
  // TODO: include refs into build()
  T::build(branch_factor, Arc::new(rows))
}

fn find_separation<X>(amin: &X, amax: &X, bmin: &X, bmax: &X, is_min: bool) -> X where X: Scalar {
  if is_min && intersect_iv(amin, amax, bmin, bmax) {
    ((*amin).clone() + (*bmin).clone()) / 2.into()
  } else if !is_min && intersect_iv(amin, amax, bmin, bmax) {
    ((*amax).clone() + (*bmax).clone()) / 2.into()
  } else {
    ((*amax).clone() + (*bmin).clone()) / 2.into()
  }
}

fn intersect_iv<X>(a0: &X, a1: &X, b0: &X, b1: &X) -> bool where X: PartialOrd {
  a1 >= b0 && a0 <= b1
}

fn intersect_pivot<X>(c: &Coord<X>, p: &X) -> bool where X: Scalar {
  match c {
    Coord::Scalar(x) => *x == *p,
    Coord::Interval(min,max) => *min <= *p && *p <= *max,
  }
}

fn intersect_coord<X>(c: &Coord<X>, low: &X, high: &X) -> bool where X: Scalar {
  match c {
    Coord::Scalar(x) => low <= x && x <= high,
    Coord::Interval(x,y) => intersect_iv(x,y,low,high),
  }
}

fn coord_cmp<X>(x: &Coord<X>, y: &Coord<X>) -> Option<std::cmp::Ordering> where X: Scalar {
  match (x,y) {
    (Coord::Scalar(a),Coord::Scalar(b)) => a.partial_cmp(b),
    (Coord::Scalar(a),Coord::Interval(b,_)) => a.partial_cmp(b),
    (Coord::Interval(a,_),Coord::Scalar(b)) => a.partial_cmp(b),
    (Coord::Interval(a,_),Coord::Interval(b,_)) => a.partial_cmp(b),
  }
}

fn coord_min<X>(x: &Coord<X>, r: &X) -> X where X: Scalar {
  let l = match x {
    Coord::Scalar(a) => a,
    Coord::Interval(a,_) => a,
  };
  match l.partial_cmp(r) {
    None => l.clone(),
    Some(std::cmp::Ordering::Less) => l.clone(),
    Some(std::cmp::Ordering::Equal) => l.clone(),
    Some(std::cmp::Ordering::Greater) => r.clone(),
  }
}

fn coord_max<X>(x: &Coord<X>, r: &X) -> X where X: Scalar {
  let l = match x {
    Coord::Scalar(a) => a,
    Coord::Interval(a,_) => a,
  };
  match l.partial_cmp(r) {
    None => l.clone(),
    Some(std::cmp::Ordering::Less) => r.clone(),
    Some(std::cmp::Ordering::Equal) => r.clone(),
    Some(std::cmp::Ordering::Greater) => l.clone(),
  }
}

fn coord_cmp_pivot<X>(x: &Coord<X>, p: &X) -> Option<std::cmp::Ordering> where X: Scalar {
  match x {
    Coord::Scalar(a) => a.partial_cmp(p),
    Coord::Interval(a,_) => a.partial_cmp(p),
  }
}

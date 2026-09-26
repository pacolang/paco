use std::collections::HashMap;

enum Shape {
    Circle(i64),
    Rect(i64, i64),
}

struct Point {
    x: i64,
    y: i64,
}

impl Point {
    fn manhattan(&self) -> i64 {
        let ax = if self.x < 0 { 0 - self.x } else { self.x };
        let ay = if self.y < 0 { 0 - self.y } else { self.y };
        ax + ay
    }
}

struct Pair<T> {
    first: T,
    second: T,
}

impl<T> Pair<T> {
    fn swap(self) -> Pair<T> {
        let Pair { first, second } = self;
        Pair { first: second, second: first }
    }
}

fn area(shape: &Shape) -> i64 {
    match shape {
        Shape::Circle(r) => 3 * r * r,
        Shape::Rect(w, h) => w * h,
    }
}

fn fib(n: i64) -> i64 {
    if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
}

fn main() {
    let mut points: Vec<Point> = Vec::new();
    for i in 0..100 {
        points.push(Point { x: i - 50, y: 2 * i - 70 });
    }
    let mut total = 0;
    for i in 0..points.len() {
        if let Some(p) = points.get(i) {
            total += p.manhattan();
        }
    }
    println!("{total}");

    let mut shapes: Vec<Shape> = Vec::new();
    for i in 1..20 {
        if i % 2 == 0 {
            shapes.push(Shape::Circle(i));
        } else {
            shapes.push(Shape::Rect(i, i + 1));
        }
    }
    let mut areas = 0;
    for i in 0..shapes.len() {
        if let Some(shape) = shapes.get(i) {
            areas += area(shape);
        }
    }
    println!("{areas}");

    let pair = Pair { first: 1, second: 2 }.swap();
    println!("{}", pair.first);
    let names = Pair { first: "left".to_string(), second: "right".to_string() }.swap();
    println!("{}", names.first);

    let mut words: HashMap<i64, String> = HashMap::new();
    words.insert(1, "one".to_string());
    words.insert(2, "two".to_string());
    match words.get(&2) {
        Some(word) => println!("{word}"),
        None => println!("missing"),
    }
    println!("{}", fib(20));
}

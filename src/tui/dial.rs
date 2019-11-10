use std::cell::RefCell;
use std::rc::Rc;

use termion::{color, cursor};

use super::Index;
use super::Observer;
use super::Scheme;
use super::{Value, get_int, get_float};
use super::Widget;

type DialRef = Rc<RefCell<Dial>>;

/** A circular dial representing a value.
 *
 * Can have logarithmic scaling to improve visibility of smaller values.
 */
pub struct Dial {
    pos_x: Index,
    pos_y: Index,
    width: Index,
    height: Index,
    min: Value,
    max: Value,
    value: Value,
    dirty: bool,
    logarithmic: bool, // Use logarithmic curve for values
    colors: Rc<Scheme>,
}

impl Dial {
    pub fn new(min: Value, max: Value, value: Value) -> DialRef {
        let pos_x: Index = 0;
        let pos_y: Index = 0;
        let width = 2;
        let height = 2;
        let dirty = false;
        let colors = Rc::new(Scheme::new());
        let logarithmic = false;
        Rc::new(RefCell::new(Dial{pos_x, pos_y, width, height, min, max, value, dirty, logarithmic, colors}))
    }

    pub fn set_logarithmic(&mut self, l: bool) {
        self.logarithmic = l;
    }

    pub fn get_index(&self, value: &Value) -> usize {
        let min: f64;
        let max: f64;
        let fvalue: f64;
        match value {
            Value::Int(v) => {
                min = get_int(&self.min) as f64;
                max = get_int(&self.max) as f64;
                fvalue = *v as f64;
            }
            Value::Float(v) => {
                min = get_float(&self.min);
                max = get_float(&self.max);
                fvalue = *v;
            }
            Value::Str(_) => panic!(),
        }
        let offset = min * -1.0;
        let range = max - min;
        let scale = 8.0 / range;
        let mut value = fvalue + offset;
        if self.logarithmic {
            // Using a logarithmic curve makes smaller values easier to see.
            let percent = value / range;
            let factor = percent.sqrt().sqrt(); // TODO: Slow, find a nicer way
            value = factor * range;
        }
        let index = (value * scale) as usize;
        index
    }
}

impl Widget for Dial {
    /** Set the dial's position.
     *
     * TODO: Check that new position is valid
     */
    fn set_position(&mut self, x: Index, y: Index) -> bool {
        self.pos_x = x;
        self.pos_y = y;
        true
    }

    /** Set dial's width.
     *
     * TODO: Check that width is valid
     */
    fn set_width(&mut self, width: Index) -> bool {
        self.width = width;
        true
    }

    /** Set dial's height.
     *
     * TODO: Check that height is valid
     */
    fn set_height(&mut self, height: Index) -> bool {
        self.height = height;
        true
    }

    fn set_dirty(&mut self, is_dirty: bool) {
        self.dirty = is_dirty;
    }

    fn set_color_scheme(&mut self, colors: Rc<Scheme>) {
        self.colors = colors;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn get_position(&self) -> (Index, Index) {
        (self.pos_x, self.pos_y)
    }

    fn get_size(&self) -> (Index, Index) {
        (self.width, self.height)
    }

    fn draw(&self) {
        let index = self.get_index(&self.value);
        // TODO: Optimize by using array
        let chars = match index {
            0 => "  ",
            1 => "  ",
            2 => "▁ ",
            3 => "\\ ",
            4 => " ▏",
            5 => " /",
            6 => " ▁",
            7 => "  ",
            _ => "  ",
            //_ => "  ",
        };
        print!("{}{}{}{}", cursor::Goto(self.pos_x, self.pos_y), color::Bg(self.colors.bg_light2), color::Fg(self.colors.fg_dark2), chars);
        let chars = match index {
            0 => "/ ",
            1 => "▔ ",
            2 => "  ",
            3 => "  ",
            4 => "  ",
            5 => "  ",
            6 => "  ",
            7 => " ▔",
            _ => " \\",
            //_ => " ▏",
        };
        print!("{}{}", cursor::Goto(self.pos_x, self.pos_y + 1), chars);
    }
}

impl Observer for Dial {
    fn update(&mut self, value: Value) {
        self.value = value;
        self.set_dirty(true);
    }
}

#[test]
fn test_dial_translation() {
    // =====
    // Float
    // =====
    // Case 1: 0.0 - 1.0
    let d = Dial::new(Value::Float(0.0), Value::Float(1.0), Value::Float(0.0));
    assert_eq!(d.borrow().get_index(&Value::Float(0.0)), 0);
    assert_eq!(d.borrow().get_index(&Value::Float(0.5)), 4);
    assert_eq!(d.borrow().get_index(&Value::Float(1.0)), 8);

    // Case 2: -1.0 - 1.0
    let d = Dial::new(Value::Float(-1.0), Value::Float(1.0), Value::Float(0.0));
    assert_eq!(d.borrow().get_index(&Value::Float(-1.0)), 0);
    assert_eq!(d.borrow().get_index(&Value::Float(0.0)), 4);
    assert_eq!(d.borrow().get_index(&Value::Float(1.0)), 8);

    // Case 3: 2.0 - 10.0
    let d = Dial::new(Value::Float(2.0), Value::Float(10.0), Value::Float(0.0));
    assert_eq!(d.borrow().get_index(&Value::Float(2.0)), 0);
    assert_eq!(d.borrow().get_index(&Value::Float(6.0)), 4);
    assert_eq!(d.borrow().get_index(&Value::Float(10.0)), 8);

    // ===
    // Int
    // ===
    // Case 1: 0 - 8
    let d = Dial::new(Value::Int(0), Value::Int(8), Value::Int(0));
    assert_eq!(d.borrow().get_index(&Value::Int(0)), 0);
    assert_eq!(d.borrow().get_index(&Value::Int(4)), 4);
    assert_eq!(d.borrow().get_index(&Value::Int(8)), 8);

    // Case 2: -4 - 4
    let d = Dial::new(Value::Int(-4), Value::Int(4), Value::Int(0));
    assert_eq!(d.borrow().get_index(&Value::Int(-4)), 0);
    assert_eq!(d.borrow().get_index(&Value::Int(0)), 4);
    assert_eq!(d.borrow().get_index(&Value::Int(4)), 8);

    // Case 3: 2 - 10
    let d = Dial::new(Value::Int(2), Value::Int(10), Value::Int(0));
    assert_eq!(d.borrow().get_index(&Value::Int(2)), 0);
    assert_eq!(d.borrow().get_index(&Value::Int(6)), 4);
    assert_eq!(d.borrow().get_index(&Value::Int(10)), 8);
}
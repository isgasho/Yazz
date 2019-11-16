mod child_widget;
mod color;
mod container;
mod controller;
mod dial;
mod label;
mod observer;
mod value;
mod widget;

extern crate termion;

use termion::{clear, cursor};

use child_widget::ChildWidget;
use color::Scheme;
use container::Container;
use controller::Controller;
use dial::Dial;
use label::Label;
use observer::Observer;
use value::{Value, get_int, get_float, get_str};
use widget::{Index, Widget, WidgetProperties};

fn main() {
    print!("{}{}", clear::All, cursor::Goto(1, 1));
    let mut controller: Controller<i32> = Controller::new();
    let mut c = Container::new(20, 20);

    let dial = Dial::new(Value::Float(0.0), Value::Float(5.0), Value::Float(3.14));
    c.add_child(dial.clone(), 2, 2);
    controller.add_observer(&1, dial);

    let label = Label::new("TestMe".to_string(), 6);
    c.add_child(label.clone(), 2, 4);
    controller.add_observer(&2, label);

    c.draw();
    controller.update(&1, Value::Float(0.99));
    controller.update(&2, Value::Str("NewTest".to_string()));
    c.set_position(1, 5);
    c.draw();
}


use rand::{prelude::SliceRandom, thread_rng, Rng};
use serde::Deserialize;

use crate::CONFIG;

#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub title: String,
    pub contrary: Option<String>,
    pub wrong: Vec<String>,
    pub correct: Vec<String>,
}

pub fn new_question() -> (&'static String, Vec<&'static String>, usize) {
    let mut rng = thread_rng();
    let question = CONFIG
        .get()
        .unwrap()
        .questions
        .choose(&mut rng)
        .expect("no question");

    let (title, correct_answers, wrong_answers) =
        if question.contrary.is_some() && rng.gen_bool(0.5) {
            (
                question.contrary.as_ref().unwrap(),
                &question.wrong,
                &question.correct,
            )
        } else {
            (&question.title, &question.correct, &question.wrong)
        };

    let correct = correct_answers.choose(&mut rng).expect("no correct answer");
    let mut options = wrong_answers
        .choose_multiple(&mut rng, 3)
        .collect::<Vec<_>>();
    let correct_idx = rng.gen_range(0..=options.len());
    options.insert(correct_idx, correct);

    (title, options, correct_idx)
}

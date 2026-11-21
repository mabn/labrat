// Unicode for the emoji names the UI offers or is likely to meet; others show as `:name:`.
export const EMOJI: Record<string, string> = {
  thumbsup: '👍', thumbsdown: '👎', smile: '😄', laughing: '😆', tada: '🎉', heart: '❤️', rocket: '🚀', eyes: '👀',
  confused: '😕', '100': '💯', clap: '👏', fire: '🔥', white_check_mark: '✅', x: '❌', thinking: '🤔', pray: '🙏',
  star: '⭐', bug: '🐛', ok_hand: '👌', slight_smile: '🙂', sweat_smile: '😅', raised_hands: '🙌', muscle: '💪',
  sunglasses: '😎', joy: '😂', cry: '😢', scream: '😱', wave: '👋', sparkles: '✨', zap: '⚡', warning: '⚠️',
  question: '❓', exclamation: '❗', heavy_check_mark: '✔️', thumbs_up: '👍', partying_face: '🥳', rofl: '🤣',
  hugging: '🤗', facepalm: '🤦', shrug: '🤷', coffee: '☕', beers: '🍻', cake: '🍰', trophy: '🏆', lock: '🔒',
  unlock: '🔓', memo: '📝', bulb: '💡', construction: '🚧', boom: '💥', see_no_evil: '🙈', smiley: '😃', grinning: '😀',
  heart_eyes: '😍', wink: '😉', blush: '😊', neutral_face: '😐', expressionless: '😑', disappointed: '😞', rage: '😡',
  skull: '💀', ghost: '👻', robot: '🤖', poop: '💩', point_up: '☝️', v: '✌️', metal: '🤘', handshake: '🤝',
};

export const PICKER = ['thumbsup', 'thumbsdown', 'smile', 'tada', 'heart', 'rocket', 'eyes', 'laughing', 'confused', '100', 'clap', 'fire', 'white_check_mark', 'x', 'thinking', 'pray', 'star', 'bug', 'ok_hand', 'slight_smile', 'sweat_smile', 'raised_hands', 'muscle', 'sunglasses', 'joy', 'cry', 'sparkles', 'zap', 'warning', 'bulb', 'memo', 'robot'];

export const emoji = (name: string) => EMOJI[name] ?? `:${name}:`;

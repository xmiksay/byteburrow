import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import HelloWorld from './HelloWorld.vue'

describe('HelloWorld', () => {
  it('renders the msg prop into the heading', () => {
    const wrapper = mount(HelloWorld, { props: { msg: 'ByteBurrow' } })
    expect(wrapper.get('h1').text()).toBe('ByteBurrow')
  })

  it('increments the counter on click', async () => {
    const wrapper = mount(HelloWorld, { props: { msg: 'hi' } })
    const button = wrapper.get('button')
    expect(button.text()).toContain('count is 0')

    await button.trigger('click')
    expect(button.text()).toContain('count is 1')
  })
})
